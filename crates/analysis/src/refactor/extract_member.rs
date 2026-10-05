//! Extract constant, field and parameter: an expression becomes a member of the class around it,
//! or a parameter of the function around it that every call is given.

use std::collections::HashSet;

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::draft::{Draft, focus};
use super::exprs::{
    expression_of_selection, expressions_at, hoist_site, is_expression, replaceable, text_slice, token_text,
    uses_class_context, variables_in,
};
use super::names::{constant_name, taken_variables, unique, variable_name};
use super::signature::{
    Family, append_item, arguments_of, document_parameter, family, is_callable_declaration, is_variadic, parameters,
};
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::actions::edits::{indent_of, insert, line_end, line_start, replace};
use crate::actions::imports::ClassWriter;
use crate::actions::members::style_of;
use crate::actions::type_text::native_type_text;
use crate::ast::{self, child_of, end, first_token, has_token, start, text_of, tokens};
use crate::completion::TextEdit;
use crate::inspections::Cx;
use crate::inspections::types::sure_type;

fn class_of(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().find(|ancestor| ast::is_class_like(ancestor.kind()))
}

/// The expression a refactor works on: the selection, or the outermost expression around the cursor that fits.
fn candidate(rcx: &Rcx<'_>, fits: impl Fn(&SyntaxNode) -> bool) -> Option<SyntaxNode> {
    let cx = &rcx.cx;
    if rcx.range.is_empty() {
        return expressions_at(&cx.root, u32::from(rcx.range.start()))
            .into_iter()
            .filter(|expr| fits(expr))
            .last();
    }
    expression_of_selection(&cx.root, cx.text, rcx.range).filter(|expr| fits(expr))
}

/// Whether the expression is one PHP accepts where only constants go: a class constant, a default.
pub(crate) fn is_constant_expression(expr: &SyntaxNode) -> bool {
    expr.descendants().all(|node| match node.kind() {
        LITERAL | NAME | BINARY_EXPR | PAREN_EXPR | ARRAY_EXPR | ARRAY_ITEM | TERNARY_EXPR | INDEX_EXPR => {
            node.kind() != BINARY_EXPR || !has_token(&node, INSTANCEOF_KW)
        }
        PREFIX_EXPR => {
            !has_token(&node, INC) && !has_token(&node, DEC) && !has_token(&node, AMP) && !has_token(&node, AT)
        }
        SCOPED_ACCESS_EXPR => node
            .children()
            .next()
            .is_none_or(|class| !text_of(&class).eq_ignore_ascii_case("static")),
        _ => false,
    })
}

/// `true`, `false` and `null`, which no name makes clearer.
fn is_trivial_name(expr: &SyntaxNode) -> bool {
    expr.kind() == NAME && matches!(text_of(expr).to_ascii_lowercase().as_str(), "true" | "false" | "null")
}

/// A bare mention of a constant, which a constant of its own would only repeat.
fn is_plain_reference(expr: &SyntaxNode) -> bool {
    match expr.kind() {
        NAME => true,
        SCOPED_ACCESS_EXPR => true,
        LITERAL => matches!(
            text_of(expr).to_ascii_lowercase().as_str(),
            "__class__" | "__function__" | "__line__" | "__file__" | "__dir__" | "__method__" | "__namespace__"
        ),
        _ => false,
    }
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    offer_constant(rcx, out);
    offer_field(rcx, out);
    offer_parameter(rcx, out);
}

// Constants -------------------------------------------------------------------------------------

fn offer_constant<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(expr) = candidate(rcx, |expr| {
        is_constant_expression(expr)
            && !is_plain_reference(expr)
            && replaceable(expr).is_ok()
            && class_of(expr).is_some()
    }) else {
        return;
    };
    let Some(class) = class_of(&expr) else {
        return;
    };
    if class.kind() == TRAIT_DECLARATION && rcx.cx.index.level < php_syntax::PhpVersion::V8_2 {
        return;
    }
    let (from, to) = (start(&expr), end(&expr));
    let single = expr.clone();
    out.push(Refactor::new(
        format!("extract-constant@{from}-{to}"),
        "Extract constant",
        RefactorKind::Extract,
        true,
        move || extract_constant(rcx, &single, false),
    ));
    let count = same_expressions(&expr, &class).len();
    if count > 1 {
        let all = expr.clone();
        out.push(Refactor::new(
            format!("extract-constant-all@{from}-{to}"),
            format!("Extract constant, replacing all {count} occurrences"),
            RefactorKind::Extract,
            true,
            move || extract_constant(rcx, &all, true),
        ));
    }
}

/// The expressions of a class that spell the same as this one, outermost only.
fn same_expressions(expr: &SyntaxNode, class: &SyntaxNode) -> Vec<SyntaxNode> {
    let wanted = token_text(expr);
    let mut found: Vec<SyntaxNode> = class
        .descendants()
        .filter(|node| node.kind() == expr.kind() && is_expression(node) && token_text(node) == wanted)
        .filter(|node| replaceable(node).is_ok())
        .collect();
    found.sort_by_key(start);
    let mut kept: Vec<SyntaxNode> = Vec::new();
    for node in found {
        if kept.last().is_some_and(|last| end(&node) <= end(last)) {
            continue;
        }
        kept.push(node);
    }
    kept
}

fn class_constant_names(rcx: &Rcx<'_>, class: &SyntaxNode) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    if let Some(body) = child_of(class, CLASS_BODY) {
        for declaration in body
            .children()
            .filter(|member| member.kind() == CLASS_CONST_DECLARATION)
        {
            for element in declaration.children().filter(|node| node.kind() == CONST_ELEMENT) {
                if let Some(name) = element.children().next() {
                    names.insert(text_of(&name));
                }
            }
        }
    }
    let analyzer = rcx.cx.file.analyzer(class);
    if let Some(context) = &analyzer.class {
        for found in rcx.cx.index.constants_of(&Type::class(context.name.clone())) {
            names.insert(found.member.name.clone());
        }
    }
    names
}

fn extract_constant(rcx: &Rcx<'_>, expr: &SyntaxNode, all: bool) -> Result<Change, String> {
    let cx = &rcx.cx;
    replaceable(expr)?;
    if !is_constant_expression(expr) {
        return Err("The expression is not a constant".to_string());
    }
    let class = class_of(expr).ok_or("The expression is not inside a class")?;
    let places = if all {
        same_expressions(expr, &class)
    } else {
        vec![expr.clone()]
    };
    let taken = class_constant_names(rcx, &class);
    let name = unique(&constant_name(expr), &taken);
    let visibility = if class.kind() == INTERFACE_DECLARATION {
        "public"
    } else {
        "private"
    };
    let value = text_slice(cx.text, expr);
    let block = format!("{visibility} const {} = {value};", focus(&name));
    let mut draft = Draft::new(rcx.renv);
    draft.here(member_insertion(cx, &class, &block, MemberGroup::Constant, false));
    for place in &places {
        draft.here(replace(place.text_range(), format!("self::{name}")));
    }
    draft.finish()
}

// Members ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum MemberGroup {
    TraitUse,
    Case,
    Constant,
    Property,
    Method,
}

pub(super) fn group_of(member: &SyntaxNode) -> MemberGroup {
    match member.kind() {
        TRAIT_USE => MemberGroup::TraitUse,
        ENUM_CASE => MemberGroup::Case,
        CLASS_CONST_DECLARATION => MemberGroup::Constant,
        PROPERTY_DECLARATION => MemberGroup::Property,
        _ => MemberGroup::Method,
    }
}

/// The edit that puts a member among the ones of its kind, with the blank lines the groups keep.
pub(super) fn member_insertion(
    cx: &Cx,
    class: &SyntaxNode,
    block: &str,
    group: MemberGroup,
    set_apart: bool,
) -> TextEdit {
    let style = style_of(cx, class);
    let eol = if cx.text.contains("\r\n") { "\r\n" } else { "\n" };
    let body = child_of(class, CLASS_BODY);
    let members: Vec<SyntaxNode> = body.iter().flat_map(|body| body.children()).collect();
    let indented = block
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{}{line}", style.indent)
            }
        })
        .collect::<Vec<_>>()
        .join(eol);
    let before = members.iter().rposition(|member| group_of(member) <= group);
    match before {
        Some(at) => {
            let after_member = line_end(cx.text, end(&members[at]) as usize);
            let rest = &cx.text[after_member..];
            let blank_len = if rest.starts_with("\r\n") {
                2
            } else {
                usize::from(rest.starts_with('\n'))
            };
            let next = members.get(at + 1);
            let other_group = group_of(&members[at]) != group || set_apart;
            let blank_after = next.is_some_and(|next| group_of(next) > group);
            if other_group && blank_len > 0 && next.is_some() {
                let trail = if blank_after { eol } else { "" };
                return insert((after_member + blank_len) as u32, format!("{indented}{eol}{trail}"));
            }
            let lead = if other_group && blank_len == 0 { eol } else { "" };
            let trail = if blank_after && blank_len == 0 { eol } else { "" };
            insert(after_member as u32, format!("{lead}{indented}{eol}{trail}"))
        }
        None => match members.first() {
            Some(first) => insert(
                line_start(cx.text, start(first) as usize) as u32,
                format!("{indented}{eol}{eol}"),
            ),
            None => {
                let open = body
                    .as_ref()
                    .and_then(|body| tokens(body).find(|token| token.kind() == LBRACE));
                let close = body
                    .as_ref()
                    .and_then(|body| tokens(body).find(|token| token.kind() == RBRACE));
                let (Some(open), Some(close)) = (open, close) else {
                    return insert(end(class), format!("{eol}{indented}{eol}"));
                };
                let after_open = u32::from(open.text_range().end()) as usize;
                let line_after = line_end(cx.text, after_open);
                if cx.text[after_open..line_after].trim().is_empty()
                    && after_open <= u32::from(close.text_range().start()) as usize
                {
                    insert(line_after as u32, format!("{indented}{eol}"))
                } else {
                    replace(
                        TextRange::new(open.text_range().end(), close.text_range().start()),
                        format!("{eol}{indented}{eol}{}", indent_of(cx.text, start(class) as usize)),
                    )
                }
            }
        },
    }
}

// Fields ----------------------------------------------------------------------------------------

fn method_of(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().find(|ancestor| ancestor.kind() == METHOD_DECLARATION)
}

fn is_static_method(method: &SyntaxNode) -> bool {
    child_of(method, MODIFIER_LIST).is_some_and(|list| has_token(&list, STATIC_KW))
}

pub(super) fn is_constructor(method: &SyntaxNode) -> bool {
    child_of(method, NAME).is_some_and(|name| text_of(&name).eq_ignore_ascii_case("__construct"))
}

fn offer_field<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(expr) = candidate(rcx, |expr| {
        !is_plain_reference(expr)
            && !matches!(expr.kind(), VARIABLE_EXPR | ASSIGN_EXPR)
            && replaceable(expr).is_ok()
            && method_of(expr).is_some_and(|method| {
                child_of(&method, BLOCK).is_some_and(|body| body.text_range().contains_range(expr.text_range()))
            })
    }) else {
        return;
    };
    let Some(class) =
        class_of(&expr).filter(|class| matches!(class.kind(), CLASS_DECLARATION | TRAIT_DECLARATION | ANONYMOUS_CLASS))
    else {
        return;
    };
    let Some(method) = method_of(&expr) else {
        return;
    };
    let (from, to) = (start(&expr), end(&expr));
    if is_constant_expression(&expr) {
        let inline = expr.clone();
        out.push(Refactor::new(
            format!("extract-field-inline@{from}-{to}"),
            "Extract field, initialized inline",
            RefactorKind::Extract,
            true,
            move || extract_field(rcx, &inline, true),
        ));
    }
    if !is_static_method(&method) && class.kind() != ANONYMOUS_CLASS
        || class.kind() == ANONYMOUS_CLASS && !is_static_method(&method)
    {
        let in_constructor = expr.clone();
        out.push(Refactor::new(
            format!("extract-field-constructor@{from}-{to}"),
            "Extract field, initialized in the constructor",
            RefactorKind::Extract,
            true,
            move || extract_field(rcx, &in_constructor, false),
        ));
    }
}

fn property_names(rcx: &Rcx<'_>, class: &SyntaxNode) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    let analyzer = rcx.cx.file.analyzer(class);
    if let Some(context) = &analyzer.class {
        for found in rcx.cx.index.properties(&Type::class(context.name.clone())) {
            names.insert(found.member.name.clone());
        }
    }
    if let Some(body) = child_of(class, CLASS_BODY) {
        for declaration in body.descendants().filter(|node| node.kind() == PROPERTY_ELEMENT) {
            if let Some(token) = first_token(&declaration, VARIABLE) {
                names.insert(token.text().trim_start_matches('$').to_string());
            }
        }
    }
    names
}

fn native_type_of(rcx: &Rcx<'_>, class: &SyntaxNode, expr: &SyntaxNode) -> Option<(String, Vec<TextEdit>)> {
    let cx = &rcx.cx;
    let analyzer = cx.file.analyzer(expr);
    let env = analyzer.env_around(expr);
    let ty = sure_type(cx, &analyzer, &env, expr)?;
    if ty.is_unknown() || matches!(ty, Type::Void | Type::Never | Type::Null | Type::Mixed) {
        return None;
    }
    let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, cx.text, &cx.root, start(class));
    let text = native_type_text(&ty, &mut |name: &str| writer.written(name))?;
    let text = super::extract_method::flatten_union(&text)?;
    if (text.contains('|') || text == "mixed") && cx.index.level < php_syntax::PhpVersion::V8_0 {
        return None;
    }
    if matches!(text.as_str(), "static" | "self" | "parent") {
        return None;
    }
    Some((text, writer.into_imports()))
}

fn extract_field(rcx: &Rcx<'_>, expr: &SyntaxNode, inline: bool) -> Result<Change, String> {
    let cx = &rcx.cx;
    replaceable(expr)?;
    let class = class_of(expr).ok_or("The expression is not inside a class")?;
    if !matches!(class.kind(), CLASS_DECLARATION | TRAIT_DECLARATION | ANONYMOUS_CLASS) {
        return Err("Only a class can have a field".to_string());
    }
    let method = method_of(expr).ok_or("The expression is not inside a method")?;
    if expr
        .ancestors()
        .take_while(|ancestor| *ancestor != method)
        .any(|ancestor| {
            matches!(ancestor.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR) && has_token(&ancestor, STATIC_KW)
        })
    {
        return Err("The expression is in a static closure, which has no object".to_string());
    }
    let is_static = is_static_method(&method);
    if inline && !is_constant_expression(expr) {
        return Err("A field can only start as a constant".to_string());
    }
    if !inline {
        if is_static {
            return Err("A static method has no constructor to initialize a field in".to_string());
        }
        let allowed: Vec<String> = if is_constructor(&method) {
            child_of(&method, PARAMETER_LIST)
                .map(|list| {
                    list.children()
                        .filter(|parameter| parameter.kind() == PARAMETER)
                        .map(|parameter| super::signature::parameter_name(&parameter))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let reads: Vec<String> = variables_in(expr).into_iter().filter(|name| name != "this").collect();
        if let Some(local) = reads.iter().find(|name| !allowed.contains(name)) {
            return Err(format!(
                "The expression uses ${local}, which the constructor does not have"
            ));
        }
    }
    let taken = property_names(rcx, &class);
    let name = unique(&variable_name(cx, expr), &taken);
    let (type_text, imports) = native_type_of(rcx, &class, expr).unzip();
    let ty = type_text.map(|ty| format!("{ty} ")).unwrap_or_default();
    let value = text_slice(cx.text, expr);
    let static_keyword = if is_static { "static " } else { "" };
    let declaration = if inline {
        format!("private {static_keyword}{ty}${} = {value};", focus(&name))
    } else {
        format!("private {static_keyword}{ty}${};", focus(&name))
    };
    let reference = if is_static {
        format!("self::${name}")
    } else {
        format!("$this->{name}")
    };
    let mut draft = Draft::new(rcx.renv);
    draft.here(replace(expr.text_range(), reference));
    let mut block = declaration;
    if inline {
        draft.here(member_insertion(cx, &class, &block, MemberGroup::Property, false));
    } else {
        let assignment = format!("$this->{name} = {value};");
        match constructor_statement(cx, &class, &assignment)? {
            Constructor::Existing(edit) => {
                draft.here(member_insertion(cx, &class, &block, MemberGroup::Property, false));
                draft.here(edit);
            }
            Constructor::New(text) => {
                let eol = if cx.text.contains("\r\n") { "\r\n" } else { "\n" };
                block.push_str(&format!("{eol}{eol}{text}"));
                draft.here(member_insertion(cx, &class, &block, MemberGroup::Property, false));
            }
        }
    }
    draft.here_all(imports.unwrap_or_default());
    draft.finish()
}

/// What adding a statement to a constructor takes.
enum Constructor {
    /// The edit that adds it to the constructor there is.
    Existing(TextEdit),
    /// The text of a constructor that has it.
    New(String),
}

fn constructor_statement(cx: &Cx, class: &SyntaxNode, statement: &str) -> Result<Constructor, String> {
    let style = style_of(cx, class);
    let eol = if cx.text.contains("\r\n") { "\r\n" } else { "\n" };
    let existing = child_of(class, CLASS_BODY).and_then(|body| {
        body.children()
            .find(|member| member.kind() == METHOD_DECLARATION && is_constructor(member))
    });
    let inner = format!("{}{}", style.indent, style.unit);
    let Some(constructor) = existing else {
        let unit = &style.unit;
        return Ok(Constructor::New(if style.brace_on_next_line {
            format!("public function __construct(){eol}{{{eol}{unit}{statement}{eol}}}")
        } else {
            format!("public function __construct() {{{eol}{unit}{statement}{eol}}}")
        }));
    };
    let Some(body) = child_of(&constructor, BLOCK) else {
        return Err("The constructor has no body".to_string());
    };
    let last = body.children().last();
    Ok(Constructor::Existing(match last {
        Some(last) => insert(
            line_end(cx.text, end(&last) as usize) as u32,
            format!("{inner}{statement}{eol}"),
        ),
        None => {
            let open = tokens(&body)
                .find(|token| token.kind() == LBRACE)
                .map(|token| token.text_range().end());
            let close = tokens(&body)
                .find(|token| token.kind() == RBRACE)
                .map(|token| token.text_range().start());
            let (Some(open), Some(close)) = (open, close) else {
                return Err("The constructor has no body".to_string());
            };
            replace(
                TextRange::new(open, close),
                format!(
                    "{eol}{inner}{statement}{eol}{}",
                    indent_of(cx.text, start(&constructor) as usize)
                ),
            )
        }
    }))
}

// Parameters ------------------------------------------------------------------------------------

fn offer_parameter<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(expr) = candidate(rcx, |expr| {
        !is_trivial_name(expr)
            && !matches!(expr.kind(), VARIABLE_EXPR | ASSIGN_EXPR)
            && replaceable(expr).is_ok()
            && function_of(expr).is_some_and(|function| {
                child_of(&function, BLOCK).is_some_and(|body| body.text_range().contains_range(expr.text_range()))
            })
            && variables_in(expr).is_empty()
            && !uses_class_context(expr)
    }) else {
        return;
    };
    let (from, to) = (start(&expr), end(&expr));
    out.push(Refactor::new(
        format!("extract-parameter@{from}-{to}"),
        "Extract parameter",
        RefactorKind::Extract,
        true,
        move || extract_parameter(rcx, &expr),
    ));
}

fn function_of(node: &SyntaxNode) -> Option<SyntaxNode> {
    let function = ast::enclosing_function(node)?;
    is_callable_declaration(&function).then_some(function)
}

/// The text of an expression where a file other than its own can read it: names written in full.
fn portable_text(rcx: &Rcx<'_>, expr: &SyntaxNode) -> String {
    let cx = &rcx.cx;
    let analyzer = cx.file.analyzer(expr);
    let base = start(expr);
    let mut text = text_slice(cx.text, expr).to_string();
    let mut names: Vec<SyntaxNode> = expr.descendants().filter(|node| node.kind() == NAME).collect();
    names.sort_by_key(|node| std::cmp::Reverse(start(node)));
    for name in names {
        let Some(replacement) = super::names::fully_qualified(cx.index, &analyzer.resolver, &name) else {
            continue;
        };
        let from = (start(&name) - base) as usize;
        let to = (end(&name) - base) as usize;
        text.replace_range(from..to, &replacement);
    }
    text
}

fn extract_parameter(rcx: &Rcx<'_>, expr: &SyntaxNode) -> Result<Change, String> {
    let cx = &rcx.cx;
    replaceable(expr)?;
    let function = function_of(expr).ok_or("The expression is not in a function")?;
    if !variables_in(expr).is_empty() || uses_class_context(expr) {
        return Err("The expression uses what only the function has".to_string());
    }
    if super::exprs::has_side_effects(expr) {
        hoist_site(expr).map_err(|reason| format!("The expression does something, so it has to stay: {reason}"))?;
        if expr.ancestors().any(|ancestor| {
            matches!(
                ancestor.kind(),
                WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT
            )
        }) {
            return Err("The expression does something and runs in a loop".to_string());
        }
    }
    let family = family(rcx, &function)?;
    if family.loose > 0 {
        return Err("The function is also used where it is not called, which this would break".to_string());
    }
    let own_params = parameters(&function);
    if own_params.iter().any(is_variadic) {
        return Err("A parameter cannot be added before a variadic one".to_string());
    }
    let optional_last = own_params
        .last()
        .is_some_and(|last| super::signature::default_of(cx.text, last).is_some());
    let constant = is_constant_expression(expr);
    if optional_last && !constant {
        return Err("A parameter without a default cannot follow ones that have".to_string());
    }
    let mut taken: HashSet<String> = HashSet::new();
    for declared in &family.declarations {
        taken.extend(taken_variables(&declared.function));
    }
    let name = unique(&variable_name(cx, expr), &taken);
    let typed_style = own_params
        .iter()
        .any(|parameter| parameter.children().any(|child| ast::is_type_node(child.kind())))
        || own_params.is_empty();
    let value_type = expression_type(rcx, expr);
    let value_here = text_slice(cx.text, expr).to_string();
    let value_elsewhere = portable_text(rcx, expr);
    let mut draft = Draft::new(rcx.renv);
    draft.here(replace(expr.text_range(), format!("${name}")));
    let default = if optional_last {
        format!(" = {value_here}")
    } else {
        String::new()
    };
    for declared in &family.declarations {
        let file = &family.files[declared.file];
        let list = child_of(&declared.function, PARAMETER_LIST).ok_or("The declaration has no parameter list")?;
        let current = file.path == rcx.renv.path && declared.function == function;
        let shown = if current { focus(&name) } else { name.clone() };
        let default_here = if file.path == rcx.renv.path {
            default.clone()
        } else if optional_last {
            format!(" = {value_elsewhere}")
        } else {
            String::new()
        };
        let (written, imports) = match (&value_type, typed_style) {
            (Some(ty), true) => written_in(rcx, file, &declared.function, ty).unzip(),
            _ => (None, None),
        };
        let prefix = written.as_ref().map(|ty| format!("{ty} ")).unwrap_or_default();
        draft.edit(
            &file.path,
            append_item(&file.text, &list, &format!("{prefix}${shown}{default_here}"), PARAMETER),
        );
        draft.edits(&file.path, imports.unwrap_or_default());
        draft.edits(
            &file.path,
            document_parameter(&file.text, &declared.function, &name, written.as_deref()),
        );
    }
    if !optional_last {
        add_arguments(
            &mut draft,
            rcx,
            &family,
            &own_params,
            &name,
            &value_here,
            &value_elsewhere,
        )?;
    }
    draft.finish()
}

/// The type of an expression, when it can be written as a declaration.
fn expression_type(rcx: &Rcx<'_>, expr: &SyntaxNode) -> Option<Type> {
    let analyzer = rcx.cx.file.analyzer(expr);
    let env = analyzer.env_around(expr);
    let ty = sure_type(&rcx.cx, &analyzer, &env, expr)?;
    (!ty.is_unknown() && !matches!(ty, Type::Void | Type::Never | Type::Null | Type::Mixed)).then_some(ty)
}

/// A type as one file writes it, with the imports that takes.
fn written_in(
    rcx: &Rcx<'_>,
    file: &super::signature::Loaded,
    function: &SyntaxNode,
    ty: &Type,
) -> Option<(String, Vec<TextEdit>)> {
    let resolver = php_index::extract::resolver_at(&file.root, start(function));
    let mut writer = ClassWriter::new(rcx.cx.index, &resolver, &file.text, &file.root, start(function));
    let text = native_type_text(ty, &mut |name: &str| writer.written(name))?;
    let text = super::extract_method::flatten_union(&text)?;
    if (text.contains('|') || text == "mixed") && rcx.cx.index.level < php_syntax::PhpVersion::V8_0 {
        return None;
    }
    if matches!(text.as_str(), "static" | "self" | "parent") {
        return None;
    }
    Some((text, writer.into_imports()))
}

fn add_arguments(
    draft: &mut Draft<'_>,
    rcx: &Rcx<'_>,
    family: &Family,
    own_params: &[SyntaxNode],
    name: &str,
    value_here: &str,
    value_elsewhere: &str,
) -> Result<(), String> {
    for site in &family.calls {
        let file = &family.files[site.file];
        let given = arguments_of(&site.call);
        if given.iter().any(|arg| arg.spread) {
            return Err(format!("A call in {} spreads its arguments", file.path.display()));
        }
        let list = child_of(&site.call, ARGUMENT_LIST).ok_or("The call has no arguments")?;
        let value = if file.path == rcx.renv.path {
            value_here
        } else {
            value_elsewhere
        };
        let positional = given.iter().take_while(|arg| arg.name.is_none()).count();
        let everything_given = positional == own_params.len() && positional == given.len();
        let item = if everything_given {
            value.to_string()
        } else if rcx.cx.index.level >= php_syntax::PhpVersion::V8_0 {
            format!("{name}: {value}")
        } else {
            return Err(format!(
                "A call in {} leaves out arguments the new one has to follow",
                file.path.display()
            ));
        };
        draft.edit(&file.path, append_item(&file.text, &list, &item, ARGUMENT));
    }
    Ok(())
}
