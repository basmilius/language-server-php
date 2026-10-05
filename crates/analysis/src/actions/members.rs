//! Members of classes: writing the methods a class must have, creating one that is called and
//! missing, and the intentions that change a member's visibility, make it readonly or promote it
//! into the constructor.

use php_index::{Method, Type, Visibility};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::edits::{indent_of, insert, line_end, line_start, remove_with_lines, replace};
use super::imports::ClassWriter;
use super::type_text::native_type_text;
use super::{Action, ActionInput, ActionKind};
use crate::ast::{self, child_of, first_token, has_token, start, text_of, tokens};
use crate::completion::TextEdit;
use crate::inspections::Cx;
use crate::inspections::classes::missing_methods;
use crate::inspections::types::sure_type;
use crate::inspections::util::member_name;
use crate::refs::is_write_target;

// Writing methods -------------------------------------------------------------------------------

/// How a file lays out its classes: the indentation of a member and where a method's brace goes.
pub(crate) struct Style {
    pub indent: String,
    pub unit: String,
    pub brace_on_next_line: bool,
}

pub(crate) fn style_of(cx: &Cx, class: &SyntaxNode) -> Style {
    let class_indent = indent_of(cx.text, usize::from(class.text_range().start()));
    let member_indent = child_of(class, CLASS_BODY)
        .and_then(|body| body.children().next())
        .map(|member| indent_of(cx.text, usize::from(member.text_range().start())))
        .filter(|indent| indent.len() > class_indent.len());
    let unit = match &member_indent {
        Some(indent) => indent[class_indent.len()..].to_string(),
        None if class_indent.contains('\t') => "\t".to_string(),
        None => "    ".to_string(),
    };
    let indent = member_indent.unwrap_or_else(|| format!("{class_indent}{unit}"));
    let brace_on_next_line = cx
        .nodes
        .iter()
        .filter(|node| node.kind() == METHOD_DECLARATION)
        .find_map(|method| {
            let body = child_of(method, BLOCK).filter(|body| body.text().contains_char('\n'))?;
            let header_end = usize::from(body.text_range().start());
            let before = &cx.text[usize::from(method.text_range().start())..header_end];
            Some(before.trim_end_matches([' ', '\t']).ends_with('\n'))
        })
        .unwrap_or(true);
    Style {
        indent,
        unit,
        brace_on_next_line,
    }
}

fn class_node_at(cx: &Cx, offset: u32) -> Option<SyntaxNode> {
    cx.nodes
        .iter()
        .find(|node| ast::is_class_like(node.kind()) && start(node) == offset)
        .cloned()
}

fn class_node_named(cx: &Cx, name: &str) -> Option<SyntaxNode> {
    cx.nodes
        .iter()
        .filter(|node| matches!(node.kind(), CLASS_DECLARATION | ENUM_DECLARATION | TRAIT_DECLARATION))
        .find(|node| {
            let analyzer = cx.file.analyzer(node);
            child_of(node, NAME)
                .is_some_and(|class| analyzer.resolver.qualify(&text_of(&class)).eq_ignore_ascii_case(name))
        })
        .cloned()
}

/// The edit that adds text at the end of a class body, a blank line after the last member.
fn append_to_class(cx: &Cx, class: &SyntaxNode, blocks: &[String]) -> Option<TextEdit> {
    let body = child_of(class, CLASS_BODY)?;
    let closing = tokens(&body).find(|token| token.kind() == RBRACE)?;
    let opening = tokens(&body).find(|token| token.kind() == LBRACE)?;
    let close_at = usize::from(closing.text_range().start());
    let has_members = body.children().next().is_some();
    let joined = blocks.join("\n\n");
    let closing_line_start = line_start(cx.text, close_at);
    let alone = cx.text[closing_line_start..close_at].trim().is_empty();
    if alone {
        let text = if has_members {
            format!("\n{joined}\n")
        } else {
            format!("{joined}\n")
        };
        return Some(insert(closing_line_start as u32, text));
    }
    let class_indent = indent_of(cx.text, usize::from(class.text_range().start()));
    let range = TextRange::new(opening.text_range().end(), closing.text_range().start());
    let leading = if has_members { "\n\n" } else { "\n" };
    Some(replace(range, format!("{leading}{joined}\n{class_indent}")))
}

struct Body<'a> {
    name: &'a str,
    /// The method says what it returns, so a body that does nothing is not enough.
    returns_value: bool,
}

#[allow(clippy::too_many_arguments)]
fn method_text(
    writer: &mut ClassWriter,
    level: php_syntax::PhpVersion,
    method: &Method,
    visibility: Visibility,
    style: &Style,
    body: &Body,
    force_return: Option<&str>,
) -> String {
    let mut class = |name: &str| writer.written(name);
    let params: Vec<String> = method
        .callable
        .params_at(level)
        .map(|param| {
            let mut out = String::new();
            if let Some(text) = param.native_type(level).and_then(|ty| native_type_text(ty, &mut class)) {
                out.push_str(&text);
                out.push(' ');
            }
            if param.by_ref {
                out.push('&');
            }
            if param.variadic {
                out.push_str("...");
            }
            out.push('$');
            out.push_str(&param.name);
            if let Some(default) = &param.default {
                out.push_str(" = ");
                out.push_str(default);
            }
            out
        })
        .collect();
    let ret = force_return.map(str::to_string).or_else(|| {
        method
            .callable
            .native_return(level)
            .and_then(|ty| native_type_text(ty, &mut class))
    });
    let mut header = String::new();
    header.push_str(visibility.keyword());
    header.push(' ');
    if method.is_static {
        header.push_str("static ");
    }
    header.push_str(&format!("function {}({})", method.name, params.join(", ")));
    if let Some(ret) = &ret {
        header.push_str(&format!(": {ret}"));
    }
    let Style {
        indent,
        unit,
        brace_on_next_line,
    } = style;
    let line = if body.returns_value || ret.as_deref().is_some_and(|ret| ret != "void") {
        "throw new \\LogicException('Not implemented.');".to_string()
    } else {
        format!("// TODO: Implement {}() method.", body.name)
    };
    if *brace_on_next_line {
        format!("{indent}{header}\n{indent}{{\n{indent}{unit}{line}\n{indent}}}")
    } else {
        format!("{indent}{header} {{\n{indent}{unit}{line}\n{indent}}}")
    }
}

/// Writes out every method a class must have and does not.
pub(super) fn implement_missing(cx: &Cx, class_start: u32) -> Vec<TextEdit> {
    let Some(class) = class_node_at(cx, class_start) else {
        return Vec::new();
    };
    let analyzer = cx.file.analyzer(&class);
    let Some(name_node) = child_of(&class, NAME) else {
        return Vec::new();
    };
    let name = analyzer.resolver.qualify(&text_of(&name_node));
    let missing = missing_methods(cx, &name);
    if missing.is_empty() {
        return Vec::new();
    }
    let style = style_of(cx, &class);
    let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, cx.text, &cx.root, class_start);
    let blocks: Vec<String> = missing
        .iter()
        .map(|entry| {
            let body = Body {
                name: &entry.method.name,
                returns_value: false,
            };
            method_text(
                &mut writer,
                cx.index.level,
                &entry.method,
                entry.method.visibility,
                &style,
                &body,
                None,
            )
        })
        .collect();
    let mut edits: Vec<TextEdit> = append_to_class(cx, &class, &blocks).into_iter().collect();
    edits.extend(writer.into_imports());
    edits
}

// Creating members ------------------------------------------------------------------------------

/// A method or property that code uses and the class does not have, on a class of this file.
pub(super) fn create_member(
    cx: &Cx,
    class_name: &str,
    name: &str,
    is_static: bool,
    is_method: bool,
    at: u32,
) -> Vec<TextEdit> {
    let Some(class) = class_node_named(cx, class_name) else {
        return Vec::new();
    };
    let Some(access) = cx.nodes.iter().find(|node| {
        matches!(node.kind(), PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR)
            && start(node) == at
            && member_name(node).is_some_and(|member| text_of(&member) == name)
    }) else {
        return Vec::new();
    };
    let first = access.children().next();
    let from_inside = first.as_ref().is_some_and(|first| {
        let text = text_of(first).to_ascii_lowercase();
        matches!(text.as_str(), "$this" | "self" | "static")
    });
    let visibility = if from_inside {
        Visibility::Private
    } else {
        Visibility::Public
    };
    let style = style_of(cx, &class);
    let analyzer = cx.file.analyzer(&class);
    let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, cx.text, &cx.root, start(&class));
    let call_analyzer = cx.file.analyzer(access);
    let env = call_analyzer.env_around(access);
    let mut edits = Vec::new();
    if is_method {
        let call = access.parent().filter(|parent| parent.kind() == CALL_EXPR);
        let args = call.as_ref().map(crate::infer::arguments).unwrap_or_default();
        let mut used: Vec<String> = Vec::new();
        let mut params: Vec<String> = Vec::new();
        for (position, arg) in args.iter().enumerate() {
            let candidate = arg.name.clone().unwrap_or_else(|| match arg.expr.as_ref() {
                Some(expr) if expr.kind() == VARIABLE_EXPR => text_of(expr).trim_start_matches('$').to_string(),
                Some(expr) if expr.kind() == PROPERTY_FETCH_EXPR => member_name(expr)
                    .map(|member| text_of(&member))
                    .unwrap_or_else(|| format!("arg{}", position + 1)),
                _ => format!("arg{}", position + 1),
            });
            let mut unique = candidate.clone();
            let mut counter = 2;
            while used.contains(&unique) {
                unique = format!("{candidate}{counter}");
                counter += 1;
            }
            used.push(unique.clone());
            let mut write_class = |class_name: &str| writer.written(class_name);
            let ty = arg
                .expr
                .as_ref()
                .and_then(|expr| sure_type(cx, &call_analyzer, &env, expr))
                .filter(|ty| *ty != Type::Null)
                .and_then(|ty| native_type_text(&widen(&ty), &mut write_class));
            params.push(match ty {
                Some(ty) => format!("{ty} ${unique}"),
                None => format!("${unique}"),
            });
        }
        let as_statement = call
            .as_ref()
            .and_then(|call| call.parent())
            .is_some_and(|parent| parent.kind() == EXPR_STATEMENT);
        let ret = as_statement.then_some("void");
        let static_word = if is_static { "static " } else { "" };
        let Style {
            indent,
            unit,
            brace_on_next_line,
        } = &style;
        let body_line = if as_statement {
            format!("// TODO: Implement {name}() method.")
        } else {
            "throw new \\LogicException('Not implemented.');".to_string()
        };
        let mut header = format!(
            "{} {static_word}function {name}({})",
            visibility.keyword(),
            params.join(", ")
        );
        if let Some(ret) = ret {
            header.push_str(&format!(": {ret}"));
        }
        let text = if *brace_on_next_line {
            format!("{indent}{header}\n{indent}{{\n{indent}{unit}{body_line}\n{indent}}}")
        } else {
            format!("{indent}{header} {{\n{indent}{unit}{body_line}\n{indent}}}")
        };
        edits.extend(append_to_class(cx, &class, &[text]));
    } else {
        let assigned = access
            .parent()
            .filter(|parent| parent.kind() == ASSIGN_EXPR && parent.children().next().as_ref() == Some(access))
            .and_then(|assign| assign.children().last())
            .and_then(|value| sure_type(cx, &call_analyzer, &env, &value));
        let mut write_class = |class_name: &str| writer.written(class_name);
        let ty = assigned.and_then(|ty| native_type_text(&widen(&ty), &mut write_class));
        let line = match ty {
            Some(ty) => format!("{}{} {ty} ${name};", style.indent, visibility.keyword()),
            None => format!("{}{} ${name};", style.indent, visibility.keyword()),
        };
        edits.extend(insert_property(cx, &class, &line));
    }
    edits.extend(writer.into_imports());
    edits
}

/// A literal's type says `int`, not the one number it happened to be.
fn widen(ty: &Type) -> Type {
    match ty {
        Type::IntLiteral(_) => Type::Int,
        Type::StringLiteral(_) => Type::String,
        Type::True | Type::False => Type::Bool,
        Type::Union(members) => Type::union(members.iter().map(widen)),
        other => other.clone(),
    }
}

/// A property line goes after the last property of the class, else at the top of its body.
fn insert_property(cx: &Cx, class: &SyntaxNode, line: &str) -> Option<TextEdit> {
    let body = child_of(class, CLASS_BODY)?;
    let anchor = body
        .children()
        .filter(|member| matches!(member.kind(), PROPERTY_DECLARATION | TRAIT_USE))
        .last();
    if let Some(anchor) = anchor {
        let at = line_end(cx.text, usize::from(anchor.text_range().end()));
        return Some(insert(at as u32, format!("{line}\n")));
    }
    let opening = tokens(&body).find(|token| token.kind() == LBRACE)?;
    let at = line_end(cx.text, usize::from(opening.text_range().end()));
    let has_members = body.children().next().is_some();
    let text = if has_members {
        format!("{line}\n\n")
    } else {
        format!("{line}\n")
    };
    Some(insert(at as u32, text))
}

// Visibility ------------------------------------------------------------------------------------

pub(super) fn visibility_edit(cx: &Cx, modifiers: TextRange, to: &str) -> Vec<TextEdit> {
    let node = match cx.root.covering_element(modifiers) {
        php_syntax::SyntaxElement::Node(node) => Some(node),
        php_syntax::SyntaxElement::Token(token) => token.parent(),
    };
    let Some(list) = node.and_then(|node| node.ancestors().find(|ancestor| ancestor.kind() == MODIFIER_LIST)) else {
        return Vec::new();
    };
    set_visibility(&list, to).into_iter().collect()
}

fn set_visibility(list: &SyntaxNode, to: &str) -> Option<TextEdit> {
    let token = tokens(list).find(|token| matches!(token.kind(), PUBLIC_KW | PROTECTED_KW | PRIVATE_KW))?;
    Some(replace(token.text_range(), to))
}

/// The word that starts a declaration's modifiers or, without any, what the modifiers would go before.
fn modifier_anchor(declaration: &SyntaxNode) -> Option<u32> {
    if let Some(list) = child_of(declaration, MODIFIER_LIST) {
        return Some(start(&list));
    }
    declaration
        .children_with_tokens()
        .find(|element| !element.kind().is_trivia() && !matches!(element.kind(), DOC_COMMENT | ATTRIBUTE_LIST))
        .map(|element| u32::from(element.text_range().start()))
}

pub(super) fn intentions<'a>(input: &'a ActionInput<'a>, cx: &'a Cx<'a>, out: &mut Vec<Action<'a>>) {
    let offset = u32::from(input.range.start());
    let node = ast::node_at(&cx.root, offset);
    let Some(member) = node.ancestors().find(|ancestor| {
        matches!(
            ancestor.kind(),
            METHOD_DECLARATION | PROPERTY_DECLARATION | CLASS_CONST_DECLARATION
        )
    }) else {
        return;
    };
    let in_class = member.parent().is_some_and(|body| body.kind() == CLASS_BODY);
    if !in_class || !in_header(&member, offset) {
        return;
    }
    let parameter = node.ancestors().find(|ancestor| ancestor.kind() == PARAMETER);
    visibility_intentions(&member, parameter.as_ref(), out);
    if member.kind() == PROPERTY_DECLARATION {
        make_readonly(cx, &member, out);
    }
    if let Some(parameter) = parameter {
        if member.kind() == METHOD_DECLARATION {
            promote_parameter(cx, &member, &parameter, out);
            make_promoted_readonly(cx, &member, &parameter, out);
        }
    }
}

/// The cursor is on the declaration and not inside a body that belongs to it.
fn in_header(member: &SyntaxNode, offset: u32) -> bool {
    match child_of(member, BLOCK) {
        Some(body) => offset <= start(&body),
        None => true,
    }
}

fn visibility_intentions<'a>(member: &SyntaxNode, parameter: Option<&SyntaxNode>, out: &mut Vec<Action<'a>>) {
    // A promoted parameter has its modifiers on the parameter.
    let holder = match parameter {
        Some(parameter) if child_of(parameter, MODIFIER_LIST).is_some() => parameter.clone(),
        _ => member.clone(),
    };
    let current = child_of(&holder, MODIFIER_LIST).and_then(|list| {
        tokens(&list).find_map(|token| match token.kind() {
            PUBLIC_KW => Some("public"),
            PROTECTED_KW => Some("protected"),
            PRIVATE_KW => Some("private"),
            _ => None,
        })
    });
    if holder.kind() == PARAMETER && current.is_none() {
        return;
    }
    let current = current.unwrap_or("public");
    for target in ["public", "protected", "private"] {
        if target == current {
            continue;
        }
        let holder = holder.clone();
        out.push(Action {
            id: format!("visibility:{target}@{}", start(&holder)),
            title: format!("Make it {target}"),
            kind: ActionKind::RefactorRewrite,
            preferred: false,
            finding: None,
            expensive: false,
            edits: Box::new(move || match child_of(&holder, MODIFIER_LIST) {
                Some(list) => set_visibility(&list, target).into_iter().collect(),
                None => modifier_anchor(&holder)
                    .map(|at| insert(at, format!("{target} ")))
                    .into_iter()
                    .collect(),
            }),
        });
    }
}

// Readonly and promotion ------------------------------------------------------------------------

/// How code in a class treats `$this->name`: whether it is only written in the constructor.
struct Writes {
    in_constructor: usize,
    elsewhere: usize,
}

fn writes_to(class: &SyntaxNode, property: &str) -> Writes {
    let mut writes = Writes {
        in_constructor: 0,
        elsewhere: 0,
    };
    for node in class.descendants().filter(|node| node.kind() == PROPERTY_FETCH_EXPR) {
        let is_this = node
            .children()
            .next()
            .is_some_and(|object| object.kind() == VARIABLE_EXPR && text_of(&object) == "$this");
        if !is_this || member_name(&node).is_none_or(|name| text_of(&name) != property) || !is_write_target(&node) {
            continue;
        }
        let plain_in_constructor = node.parent().is_some_and(|parent| {
            parent.kind() == ASSIGN_EXPR
                && has_token(&parent, ASSIGN)
                && parent.children().next().as_ref() == Some(&node)
        }) && node
            .ancestors()
            .find(|ancestor| ancestor.kind() == METHOD_DECLARATION)
            .and_then(|method| child_of(&method, NAME))
            .is_some_and(|name| text_of(&name).eq_ignore_ascii_case("__construct"));
        if plain_in_constructor {
            writes.in_constructor += 1;
        } else {
            writes.elsewhere += 1;
        }
    }
    writes
}

fn make_readonly<'a>(cx: &'a Cx<'a>, declaration: &SyntaxNode, out: &mut Vec<Action<'a>>) {
    if cx.index.level < php_syntax::PhpVersion::V8_1 {
        return;
    }
    let Some(modifiers) = child_of(declaration, MODIFIER_LIST) else {
        return;
    };
    let kinds: Vec<SyntaxKind> = tokens(&modifiers).map(|token| token.kind()).collect();
    let typed = declaration.children().any(|child| ast::is_type_node(child.kind()));
    let elements: Vec<SyntaxNode> = declaration
        .children()
        .filter(|child| child.kind() == PROPERTY_ELEMENT)
        .collect();
    let [element] = elements.as_slice() else {
        return;
    };
    let has_default = tokens(element).any(|token| token.kind() == ASSIGN);
    if !kinds.contains(&PRIVATE_KW)
        || kinds.contains(&READONLY_KW)
        || kinds.contains(&STATIC_KW)
        || !typed
        || has_default
    {
        return;
    }
    let Some(class) = declaration
        .ancestors()
        .find(|ancestor| ast::is_class_like(ancestor.kind()))
    else {
        return;
    };
    if class.kind() != CLASS_DECLARATION
        || child_of(&class, MODIFIER_LIST).is_some_and(|list| has_token(&list, READONLY_KW))
    {
        return;
    }
    let Some(variable) = first_token(element, VARIABLE) else {
        return;
    };
    let name = variable.text().trim_start_matches('$').to_string();
    let writes = writes_to(&class, &name);
    if writes.elsewhere > 0 || writes.in_constructor != 1 {
        return;
    }
    let Some(private) = tokens(&modifiers).find(|token| token.kind() == PRIVATE_KW) else {
        return;
    };
    let at = u32::from(private.text_range().end());
    out.push(Action {
        id: format!("readonly@{at}"),
        title: format!("Make '${name}' readonly"),
        kind: ActionKind::RefactorRewrite,
        preferred: false,
        finding: None,
        expensive: false,
        edits: Box::new(move || vec![insert(at, " readonly")]),
    });
}

fn make_promoted_readonly<'a>(cx: &'a Cx<'a>, method: &SyntaxNode, parameter: &SyntaxNode, out: &mut Vec<Action<'a>>) {
    if cx.index.level < php_syntax::PhpVersion::V8_1 {
        return;
    }
    let Some(modifiers) = child_of(parameter, MODIFIER_LIST) else {
        return;
    };
    let is_constructor = child_of(method, NAME).is_some_and(|name| text_of(&name).eq_ignore_ascii_case("__construct"));
    let kinds: Vec<SyntaxKind> = tokens(&modifiers).map(|token| token.kind()).collect();
    if !is_constructor || !kinds.contains(&PRIVATE_KW) || kinds.contains(&READONLY_KW) {
        return;
    }
    if !parameter.children().any(|child| ast::is_type_node(child.kind())) {
        return;
    }
    let Some(class) = method.ancestors().find(|ancestor| ast::is_class_like(ancestor.kind())) else {
        return;
    };
    let Some(variable) = first_token(parameter, VARIABLE) else {
        return;
    };
    let name = variable.text().trim_start_matches('$').to_string();
    let writes = writes_to(&class, &name);
    if writes.elsewhere > 0 || writes.in_constructor > 0 {
        return;
    }
    let Some(private) = tokens(&modifiers).find(|token| token.kind() == PRIVATE_KW) else {
        return;
    };
    let at = u32::from(private.text_range().end());
    out.push(Action {
        id: format!("readonly@{at}"),
        title: format!("Make '${name}' readonly"),
        kind: ActionKind::RefactorRewrite,
        preferred: false,
        finding: None,
        expensive: false,
        edits: Box::new(move || vec![insert(at, " readonly")]),
    });
}

/// `$this->x = $x;` in a constructor and a property `x`: they can be one promoted parameter.
fn promote_parameter<'a>(cx: &'a Cx<'a>, method: &SyntaxNode, parameter: &SyntaxNode, out: &mut Vec<Action<'a>>) {
    if cx.index.level < php_syntax::PhpVersion::V8_0 || child_of(parameter, MODIFIER_LIST).is_some() {
        return;
    }
    let is_constructor = child_of(method, NAME).is_some_and(|name| text_of(&name).eq_ignore_ascii_case("__construct"));
    let (Some(body), Some(class)) = (
        child_of(method, BLOCK),
        method.ancestors().find(|ancestor| ast::is_class_like(ancestor.kind())),
    ) else {
        return;
    };
    if !is_constructor || has_token(parameter, ELLIPSIS) || has_token(parameter, AMP) {
        return;
    }
    let Some(variable) = first_token(parameter, VARIABLE) else {
        return;
    };
    let variable_text = variable.text().to_string();
    let name = variable_text.trim_start_matches('$').to_string();
    let Some(assignment) = body
        .children()
        .find(|statement| is_promotable_assignment(statement, &name, &variable_text))
    else {
        return;
    };
    let Some((declaration, element)) = property_declaration(&class, &name) else {
        return;
    };
    let Some(modifiers) = child_of(&declaration, MODIFIER_LIST) else {
        return;
    };
    let kinds: Vec<SyntaxKind> = tokens(&modifiers).map(|token| token.kind()).collect();
    if kinds.contains(&STATIC_KW) || tokens(&element).any(|token| token.kind() == ASSIGN) {
        return;
    }
    let property_type = declaration.children().find(|child| ast::is_type_node(child.kind()));
    let parameter_type = parameter.children().find(|child| ast::is_type_node(child.kind()));
    if let (Some(left), Some(right)) = (&property_type, &parameter_type) {
        if text_of(left).replace(' ', "") != text_of(right).replace(' ', "") {
            return;
        }
    } else if property_type.is_some() && parameter_type.is_none() {
        return;
    }
    // Other code that writes the property would write the promoted one: that is the same thing.
    let modifier_text: Vec<String> = tokens(&modifiers)
        .filter(|token| !token.kind().is_trivia())
        .map(|token| token.text().to_string())
        .collect();
    let (parameter, declaration, assignment) = (parameter.clone(), declaration.clone(), assignment.clone());
    let promoted_start = start(&parameter);
    out.push(Action {
        id: format!("promote@{promoted_start}"),
        title: "Convert to constructor property promotion".to_string(),
        kind: ActionKind::RefactorRewrite,
        preferred: false,
        finding: None,
        expensive: false,
        edits: Box::new(move || {
            let insert_at = parameter
                .children_with_tokens()
                .find(|element| !element.kind().is_trivia() && element.kind() != ATTRIBUTE_LIST)
                .map_or(promoted_start, |element| u32::from(element.text_range().start()));
            vec![
                insert(insert_at, format!("{} ", modifier_text.join(" "))),
                remove_with_lines(cx.text, assignment.text_range()),
                remove_with_lines(cx.text, declaration.text_range()),
            ]
        }),
    });
}

fn is_promotable_assignment(statement: &SyntaxNode, property: &str, variable: &str) -> bool {
    if statement.kind() != EXPR_STATEMENT {
        return false;
    }
    let Some(assign) = statement.children().next().filter(|child| child.kind() == ASSIGN_EXPR) else {
        return false;
    };
    if !has_token(&assign, ASSIGN) || tokens(&assign).any(|token| token.kind() == AMP) {
        return false;
    }
    let mut parts = assign.children();
    let (Some(target), Some(value)) = (parts.next(), parts.next()) else {
        return false;
    };
    target.kind() == PROPERTY_FETCH_EXPR
        && target
            .children()
            .next()
            .is_some_and(|object| object.kind() == VARIABLE_EXPR && text_of(&object) == "$this")
        && member_name(&target).is_some_and(|name| text_of(&name) == property)
        && value.kind() == VARIABLE_EXPR
        && text_of(&value) == variable
}

fn property_declaration(class: &SyntaxNode, name: &str) -> Option<(SyntaxNode, SyntaxNode)> {
    let body = child_of(class, CLASS_BODY)?;
    for declaration in body.children().filter(|child| child.kind() == PROPERTY_DECLARATION) {
        let elements: Vec<SyntaxNode> = declaration
            .children()
            .filter(|child| child.kind() == PROPERTY_ELEMENT)
            .collect();
        if let [element] = elements.as_slice() {
            if first_token(element, VARIABLE).is_some_and(|token| token.text().trim_start_matches('$') == name) {
                return Some((declaration.clone(), element.clone()));
            }
        }
    }
    None
}
