//! Signatures and doc comments: adding a return type from what a function returns, writing the
//! doc block a declaration lacks and the tags one has not caught up with.

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::edits::{indent_of, insert, line_end, line_start};
use super::imports::ClassWriter;
use super::type_text::native_type_text;
use super::{Action, ActionInput, ActionKind};
use crate::ast::{self, child_of, first_token, start, text_of, tokens};
use crate::completion::TextEdit;
use crate::doc_refs::{DocItemKind, doc_items};
use crate::inspections::flow::body_completes;
use crate::inspections::types::callable_of;
use crate::inspections::{Cx, InspectionEnv};

pub(super) fn intentions<'a>(input: &'a ActionInput<'a>, cx: &'a Cx<'a>, out: &mut Vec<Action<'a>>) {
    let offset = u32::from(input.range.start());
    let node = ast::node_at(&cx.root, offset);
    let Some(declaration) = node.ancestors().find(|ancestor| {
        matches!(
            ancestor.kind(),
            FUNCTION_DECLARATION
                | METHOD_DECLARATION
                | CLOSURE_EXPR
                | ARROW_FUNCTION_EXPR
                | PROPERTY_DECLARATION
                | CLASS_CONST_DECLARATION
                | CLASS_DECLARATION
                | INTERFACE_DECLARATION
                | TRAIT_DECLARATION
                | ENUM_DECLARATION
        )
    }) else {
        return;
    };
    if !in_header(&declaration, offset) {
        return;
    }
    if matches!(
        declaration.kind(),
        FUNCTION_DECLARATION | METHOD_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR
    ) {
        return_type_intention(cx, &declaration, out);
    }
    if declaration.kind() != CLOSURE_EXPR && declaration.kind() != ARROW_FUNCTION_EXPR {
        phpdoc_intentions(cx, &declaration, out);
    }
    strict_types_intention(cx, offset, out);
}

fn in_header(declaration: &SyntaxNode, offset: u32) -> bool {
    let body = child_of(declaration, BLOCK).or_else(|| child_of(declaration, CLASS_BODY));
    match body {
        Some(body) => offset <= start(&body),
        None => true,
    }
}

fn strict_types_intention<'a>(cx: &'a Cx<'a>, offset: u32, out: &mut Vec<Action<'a>>) {
    if cx.strict_types || line_of(cx.text, offset) > 2 {
        return;
    }
    let Some(open) = cx
        .root
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == OPEN_TAG)
    else {
        return;
    };
    if cx
        .nodes
        .iter()
        .all(|node| matches!(node.kind(), SOURCE_FILE | EMPTY_STATEMENT))
    {
        return;
    }
    let at = u32::from(open.text_range().end());
    out.push(Action {
        id: "strict-types-intention".to_string(),
        title: "Add declare(strict_types=1)".to_string(),
        kind: ActionKind::RefactorRewrite,
        preferred: false,
        finding: None,
        expensive: false,
        edits: Box::new(move || {
            let rest = &cx.text[at as usize..];
            let blank_follows = rest.starts_with('\n') || rest.starts_with("\r\n");
            let text = if blank_follows {
                "\ndeclare(strict_types=1);\n"
            } else {
                "\ndeclare(strict_types=1);\n\n"
            };
            vec![insert(at, text)]
        }),
    });
}

fn line_of(text: &str, offset: u32) -> usize {
    text[..(offset as usize).min(text.len())].matches('\n').count()
}

// Return types ----------------------------------------------------------------------------------

fn return_type_intention<'a>(cx: &'a Cx<'a>, function: &SyntaxNode, out: &mut Vec<Action<'a>>) {
    if child_of(function, RETURN_TYPE).is_some() {
        return;
    }
    let Some(list) = child_of(function, PARAMETER_LIST) else {
        return;
    };
    if function.kind() == METHOD_DECLARATION
        && child_of(function, NAME).is_some_and(|name| {
            matches!(
                text_of(&name).to_ascii_lowercase().as_str(),
                "__construct" | "__destruct"
            )
        })
    {
        return;
    }
    let function = function.clone();
    let after = child_of(&function, CLOSURE_USE).map_or(list.text_range().end(), |uses| uses.text_range().end());
    let at = u32::from(after);
    let Some(label) = inferred_return(cx, &function, &mut |name: &str| crate::short(name).to_string()) else {
        return;
    };
    out.push(Action {
        id: format!("return-type@{at}"),
        title: format!("Add return type '{label}'"),
        kind: ActionKind::RefactorRewrite,
        preferred: false,
        finding: None,
        expensive: true,
        edits: Box::new(move || {
            let analyzer = cx.file.analyzer(&function);
            let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, cx.text, &cx.root, start(&function));
            let Some(text) = inferred_return(cx, &function, &mut |name: &str| writer.written(name)) else {
                return Vec::new();
            };
            let mut edits = vec![insert(at, format!(": {text}"))];
            edits.extend(writer.into_imports());
            edits
        }),
    });
}

/// What a function returns, as a native type, when every way out of it says.
fn inferred_return(cx: &Cx, function: &SyntaxNode, class: &mut dyn FnMut(&str) -> String) -> Option<String> {
    let analyzer = cx.file.analyzer(function);
    let values: Vec<Option<SyntaxNode>>;
    let completes;
    if function.kind() == ARROW_FUNCTION_EXPR {
        let value = function
            .children()
            .filter(|child| !matches!(child.kind(), PARAMETER_LIST | RETURN_TYPE))
            .last()?;
        values = vec![Some(value)];
        completes = false;
    } else {
        let body = child_of(function, BLOCK)?;
        if contains_yield(&body) {
            return None;
        }
        values = returns_of(&body);
        completes = body_completes(cx, &body);
    }
    if values.iter().all(Option::is_none) {
        return Some("void".to_string());
    }
    let mut types: Vec<Type> = Vec::new();
    for value in &values {
        match value {
            Some(expr) => {
                let env = analyzer.env_around(expr);
                let ty = widen(&analyzer.type_of(expr, &env));
                if !is_writable(&ty) {
                    return None;
                }
                types.push(ty);
            }
            None => types.push(Type::Null),
        }
    }
    if completes {
        types.push(Type::Null);
    }
    let ty = Type::union(types);
    if matches!(ty, Type::Never | Type::Null) {
        return None;
    }
    native_type_text(&ty, class)
}

fn is_writable(ty: &Type) -> bool {
    ty.members().iter().all(|member| {
        matches!(
            member,
            Type::Int
                | Type::Float
                | Type::String
                | Type::Bool
                | Type::Null
                | Type::Array(..)
                | Type::List(_)
                | Type::Shape(_)
                | Type::Object
                | Type::Static
                | Type::Class { .. }
                | Type::Iterable(..)
        )
    })
}

fn widen(ty: &Type) -> Type {
    match ty {
        Type::IntLiteral(_) => Type::Int,
        Type::StringLiteral(_) => Type::String,
        Type::True | Type::False => Type::Bool,
        Type::Union(members) => Type::union(members.iter().map(widen)),
        other => other.clone(),
    }
}

fn contains_yield(body: &SyntaxNode) -> bool {
    walk_own(body, &mut |node| matches!(node.kind(), YIELD_EXPR | YIELD_FROM_EXPR))
}

/// Visits the nodes of a body that belong to it and not to a function inside it.
fn walk_own(body: &SyntaxNode, found: &mut dyn FnMut(&SyntaxNode) -> bool) -> bool {
    let mut preorder = body.preorder();
    while let Some(event) = preorder.next() {
        let php_syntax::WalkEvent::Enter(node) = event else {
            continue;
        };
        if matches!(
            node.kind(),
            FUNCTION_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | ANONYMOUS_CLASS | CLASS_DECLARATION
        ) {
            preorder.skip_subtree();
            continue;
        }
        if found(&node) {
            return true;
        }
    }
    false
}

fn returns_of(body: &SyntaxNode) -> Vec<Option<SyntaxNode>> {
    let mut out = Vec::new();
    walk_own(body, &mut |node| {
        if node.kind() == RETURN_STATEMENT {
            out.push(node.children().next());
        }
        false
    });
    out
}

// Doc blocks ------------------------------------------------------------------------------------

fn phpdoc_intentions<'a>(cx: &'a Cx<'a>, declaration: &SyntaxNode, out: &mut Vec<Action<'a>>) {
    let doc = tokens(declaration).find(|token| token.kind() == DOC_COMMENT);
    let declaration = declaration.clone();
    match doc {
        None => {
            let at = doc_insertion(cx.text, &declaration);
            let id = format!("phpdoc@{at}");
            out.push(Action {
                id,
                title: "Add PHPDoc".to_string(),
                kind: ActionKind::RefactorRewrite,
                preferred: false,
                finding: None,
                expensive: true,
                edits: Box::new(move || generated_block(cx, &declaration)),
            });
        }
        Some(doc) => {
            if !matches!(declaration.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION) {
                return;
            }
            let missing = missing_params(cx, &declaration, &doc);
            if missing.is_empty() {
                return;
            }
            out.push(Action {
                id: format!("phpdoc-update@{}", start(&declaration)),
                title: "Update PHPDoc".to_string(),
                kind: ActionKind::RefactorRewrite,
                preferred: false,
                finding: None,
                expensive: true,
                edits: Box::new(move || add_missing_params(cx, &declaration, &doc)),
            });
        }
    }
}

/// The start of the line a declaration begins on, where a doc block goes.
fn doc_insertion(text: &str, declaration: &SyntaxNode) -> u32 {
    let first = declaration
        .children_with_tokens()
        .find(|element| !element.kind().is_trivia())
        .map_or_else(|| start(declaration), |element| u32::from(element.text_range().start()));
    line_start(text, first as usize) as u32
}

fn generated_block(cx: &Cx, declaration: &SyntaxNode) -> Vec<TextEdit> {
    let analyzer = cx.file.analyzer(declaration);
    let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, cx.text, &cx.root, start(declaration));
    let Some(lines) = tag_lines(cx, declaration, &mut writer) else {
        return Vec::new();
    };
    let at = doc_insertion(cx.text, declaration);
    let indent = indent_of(cx.text, at as usize);
    let text = match lines.as_slice() {
        [] => format!("{indent}/**\n{indent} * \n{indent} */\n"),
        [only] if declaration.kind() != METHOD_DECLARATION && declaration.kind() != FUNCTION_DECLARATION => {
            format!("{indent}/** {only} */\n")
        }
        many => {
            let body: Vec<String> = many.iter().map(|line| format!("{indent} * {line}\n")).collect();
            format!("{indent}/**\n{}{indent} */\n", body.concat())
        }
    };
    let mut edits = vec![insert(at, text)];
    edits.extend(writer.into_imports());
    edits
}

/// The tags of the doc block a declaration would get.
fn tag_lines(cx: &Cx, declaration: &SyntaxNode, writer: &mut ClassWriter) -> Option<Vec<String>> {
    match declaration.kind() {
        FUNCTION_DECLARATION | METHOD_DECLARATION => Some(function_tags(cx, declaration, writer)),
        PROPERTY_DECLARATION => {
            let ty = declaration.children().find(|child| ast::is_type_node(child.kind()))?;
            Some(vec![format!("@var {}", doc_type_of_node(cx, &ty, writer))])
        }
        CLASS_CONST_DECLARATION => Some(Vec::new()),
        _ => Some(Vec::new()),
    }
}

fn function_tags(cx: &Cx, function: &SyntaxNode, writer: &mut ClassWriter) -> Vec<String> {
    let analyzer = cx.file.analyzer(function);
    let callable = callable_of(&analyzer, function);
    let level = cx.index.level;
    let mut lines = Vec::new();
    for param in callable.params_at(level) {
        let ty = param
            .native_type(level)
            .map(|ty| doc_type(ty, writer))
            .unwrap_or_else(|| "mixed".to_string());
        let prefix = format!(
            "{}{}",
            if param.by_ref { "&" } else { "" },
            if param.variadic { "..." } else { "" }
        );
        lines.push(format!("@param {ty} {prefix}${}", param.name));
    }
    let is_constructor =
        child_of(function, NAME).is_some_and(|name| text_of(&name).eq_ignore_ascii_case("__construct"));
    if !is_constructor {
        let ret = match callable.native_return(level) {
            Some(ty) => Some(doc_type(ty, writer)),
            None => {
                let mut class = |name: &str| writer.written(name);
                inferred_return(cx, function, &mut class).or(Some("mixed".to_string()))
            }
        };
        if let Some(ret) = ret {
            lines.push(format!("@return {ret}"));
        }
    }
    let mut thrown: Vec<String> = Vec::new();
    if let Some(body) = child_of(function, BLOCK) {
        walk_own(&body, &mut |node| {
            if node.kind() == THROW_EXPR {
                if let Some(new) = node.children().next().filter(|child| child.kind() == NEW_EXPR) {
                    if let Some(name) = child_of(&new, NAME) {
                        let resolved = analyzer.resolver.resolve_class(&text_of(&name));
                        let written = writer.written(&resolved);
                        if !thrown.contains(&written) {
                            thrown.push(written);
                        }
                    }
                }
            }
            false
        });
    }
    lines.extend(thrown.into_iter().map(|name| format!("@throws {name}")));
    lines
}

/// A type as a doc comment writes it: `int|null` and not `?int`.
fn doc_type(ty: &Type, writer: &mut ClassWriter) -> String {
    match ty {
        Type::Union(members) => members
            .iter()
            .map(|member| doc_type(member, writer))
            .collect::<Vec<_>>()
            .join("|"),
        Type::Class { name, .. } => writer.written(name),
        Type::Array(..) | Type::List(_) | Type::Shape(_) => "array".to_string(),
        other => {
            let mut class = |name: &str| writer.written(name);
            native_type_text(other, &mut class).unwrap_or_else(|| "mixed".to_string())
        }
    }
}

fn doc_type_of_node(cx: &Cx, node: &SyntaxNode, writer: &mut ClassWriter) -> String {
    let analyzer = cx.file.analyzer(node);
    let scope = analyzer.class.as_ref().map(|class| php_index::extract::ClassScope {
        name: class.name.clone(),
        parent: class.parent.clone(),
        is_trait: class.kind == php_index::ClassKind::Trait,
        templates: Vec::new(),
        aliases: Vec::new(),
    });
    let mut context = php_index::phpdoc::TypeContext::new(&analyzer.resolver);
    if let Some(scope) = &scope {
        context.class_name = Some(&scope.name);
        context.parent_name = scope.parent.as_deref();
    }
    let ty = php_index::extract::native_type(node, &context);
    doc_type(&ty, writer)
}

// Tags that fell behind -------------------------------------------------------------------------

struct DocLine {
    name: String,
    /// The whole line, with its line break.
    range: TextRange,
}

fn documented_params(doc: &php_syntax::SyntaxToken, text: &str) -> Vec<DocLine> {
    let base = u32::from(doc.text_range().start());
    let mut out = Vec::new();
    let mut in_param = false;
    for item in doc_items(doc.text(), base) {
        match item.kind {
            DocItemKind::Tag(name) => in_param = name == "param",
            DocItemKind::Variable(name) if in_param => {
                in_param = false;
                let from = line_start(text, item.start as usize);
                let to = line_end(text, item.end as usize);
                out.push(DocLine {
                    name,
                    range: TextRange::new((from as u32).into(), (to as u32).into()),
                });
            }
            _ => {}
        }
    }
    out
}

fn missing_params(cx: &Cx, function: &SyntaxNode, doc: &php_syntax::SyntaxToken) -> Vec<String> {
    let documented = documented_params(doc, cx.text);
    if !doc.text().contains('\n') {
        return Vec::new();
    }
    child_of(function, PARAMETER_LIST)
        .map(|list| {
            list.children()
                .filter(|child| child.kind() == PARAMETER)
                .filter_map(|parameter| first_token(&parameter, VARIABLE))
                .map(|token| token.text().trim_start_matches('$').to_string())
                .filter(|name| documented.iter().all(|line| &line.name != name))
                .collect()
        })
        .unwrap_or_default()
}

fn add_missing_params(cx: &Cx, function: &SyntaxNode, doc: &php_syntax::SyntaxToken) -> Vec<TextEdit> {
    let documented = documented_params(doc, cx.text);
    let missing = missing_params(cx, function, doc);
    let analyzer = cx.file.analyzer(function);
    let callable = callable_of(&analyzer, function);
    let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, cx.text, &cx.root, start(function));
    let level = cx.index.level;
    let order: Vec<String> = callable.params.iter().map(|param| param.name.clone()).collect();
    let doc_start = usize::from(doc.text_range().start());
    let doc_indent = indent_of(cx.text, doc_start);
    let mut edits = Vec::new();
    for name in &missing {
        let Some(param) = callable.params.iter().find(|param| &param.name == name) else {
            continue;
        };
        let ty = param
            .native_type(level)
            .map(|ty| doc_type(ty, &mut writer))
            .unwrap_or_else(|| "mixed".to_string());
        let prefix = format!(
            "{}{}",
            if param.by_ref { "&" } else { "" },
            if param.variadic { "..." } else { "" }
        );
        let line = format!("{doc_indent} * @param {ty} {prefix}${name}\n");
        let position = order.iter().position(|candidate| candidate == name).unwrap_or(0);
        let before = order[..position]
            .iter()
            .rev()
            .find_map(|candidate| documented.iter().find(|line| &line.name == candidate));
        let after = order[position + 1..]
            .iter()
            .find_map(|candidate| documented.iter().find(|line| &line.name == candidate));
        let at = match (before, after) {
            (Some(line), _) => u32::from(line.range.end()),
            (None, Some(line)) => u32::from(line.range.start()),
            (None, None) => first_other_tag_line(cx.text, doc).unwrap_or_else(|| closing_line(cx.text, doc)),
        };
        edits.push(insert(at, line));
    }
    edits.extend(writer.into_imports());
    edits
}

/// The start of the line of the first `@return` or `@throws`, which `@param` lines go before.
fn first_other_tag_line(text: &str, doc: &php_syntax::SyntaxToken) -> Option<u32> {
    let base = u32::from(doc.text_range().start());
    doc_items(doc.text(), base)
        .into_iter()
        .find(|item| matches!(&item.kind, DocItemKind::Tag(name) if name != "param"))
        .map(|item| line_start(text, item.start as usize) as u32)
}

fn closing_line(text: &str, doc: &php_syntax::SyntaxToken) -> u32 {
    let end = usize::from(doc.text_range().end());
    line_start(text, end.saturating_sub(1)) as u32
}

/// The doc block `/**` + Enter should leave behind: the lines for the declaration below, and the
/// closing ` */`, as an edit of the empty line the cursor is on. `None` when the comment is
/// closed already or nothing follows it that has a doc block.
pub fn doc_stub_at(env: &InspectionEnv, offset: u32) -> Option<TextEdit> {
    let text = env.text;
    let cursor_line_start = line_start(text, offset as usize);
    let cursor_line_end = line_end(text, offset as usize);
    if !text[cursor_line_start..cursor_line_end].trim().is_empty() || cursor_line_start == 0 {
        return None;
    }
    let previous_start = line_start(text, cursor_line_start - 1);
    let previous = &text[previous_start..cursor_line_start];
    let opener = previous.find("/**")?;
    if previous[..opener].trim() != "" || previous[opener + 3..].trim() != "" {
        return None;
    }
    let rest = &text[cursor_line_end..];
    let upcoming = rest.trim_start();
    if upcoming.starts_with("*/") || upcoming.starts_with("* ") || upcoming.starts_with("*\n") {
        return None;
    }
    // The open comment swallows the file in the tree, so read it with the opener blanked out.
    let mut blanked = text.to_string();
    let opener_at = previous_start + opener;
    blanked.replace_range(opener_at..opener_at + 3, "   ");
    let tree = php_syntax::parse(&blanked);
    let root = tree.syntax();
    let declaration = declaration_after(&root, cursor_line_end as u32)?;
    let blanked_env = InspectionEnv {
        index: env.index,
        text: &blanked,
        root: &root,
        settings: env.settings,
        ready: env.ready,
        externals: env.externals,
    };
    let cx = Cx::new(&blanked_env);
    let analyzer = cx.file.analyzer(&declaration);
    let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, &blanked, &root, start(&declaration));
    let lines = tag_lines(&cx, &declaration, &mut writer)?;
    let indent = indent_of(text, previous_start);
    let mut out = String::new();
    if lines.is_empty() {
        out.push_str(&format!("{indent} * \n"));
    }
    for line in &lines {
        out.push_str(&format!("{indent} * {line}\n"));
    }
    out.push_str(&format!("{indent} */\n"));
    Some(TextEdit {
        start: cursor_line_start as u32,
        end: cursor_line_end as u32,
        new_text: out,
    })
}

/// The declaration that starts first after an offset, when it is the next thing in the file.
fn declaration_after(root: &SyntaxNode, offset: u32) -> Option<SyntaxNode> {
    root.descendants()
        .filter(|node| {
            matches!(
                node.kind(),
                FUNCTION_DECLARATION
                    | METHOD_DECLARATION
                    | PROPERTY_DECLARATION
                    | CLASS_CONST_DECLARATION
                    | CLASS_DECLARATION
                    | INTERFACE_DECLARATION
                    | TRAIT_DECLARATION
                    | ENUM_DECLARATION
            )
        })
        .filter(|node| {
            let first = node
                .children_with_tokens()
                .find(|element| !element.kind().is_trivia())
                .map_or_else(|| start(node), |element| u32::from(element.text_range().start()));
            first >= offset
        })
        .min_by_key(start)
}
