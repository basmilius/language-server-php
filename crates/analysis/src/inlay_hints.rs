//! Inlay hints: the name of the parameter an argument goes to, and the type a closure parameter
//! gets from the function that takes the closure.
//!
//! A parameter name is shown for every positional argument except when it says nothing the
//! argument does not: the argument is named like the parameter (`$name`, `$user->name`,
//! `getName()`), the parameter has a single letter for a name, the function takes one argument and
//! the argument is not a bare `true`, `false` or `null`, or the argument is spread or goes to a
//! variadic parameter.

use php_index::{Index, Param, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use crate::ast::{self, child_of, text_of};
use crate::context::FileContext;
use crate::infer::{Arg, ResolvedCallable, arguments};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    Parameter,
    Type,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlayHint {
    /// Where the hint is drawn, as an offset.
    pub offset: u32,
    /// The text, which for a parameter name ends in a colon.
    pub label: String,
    pub kind: HintKind,
    /// What accepting the hint writes into the file at the same offset.
    pub insert: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct HintOptions {
    pub parameter_names: bool,
    pub closure_types: bool,
}

impl Default for HintOptions {
    fn default() -> HintOptions {
        HintOptions {
            parameter_names: true,
            closure_types: true,
        }
    }
}

/// The hints for the calls whose arguments start inside a range, or in the whole file.
pub fn inlay_hints(index: &Index, root: &SyntaxNode, range: Option<TextRange>, options: HintOptions) -> Vec<InlayHint> {
    let ctx = FileContext::new(index, root);
    let mut out = Vec::new();
    for node in root.descendants() {
        if range.is_some_and(|range| range.intersect(node.text_range()).is_none()) {
            continue;
        }
        if !matches!(node.kind(), CALL_EXPR | NEW_EXPR) {
            continue;
        }
        let Some(list) = child_of(&node, ARGUMENT_LIST) else {
            continue;
        };
        if ast::has_token(&list, ELLIPSIS) && list.children().all(|child| child.kind() != ARGUMENT) {
            continue;
        }
        let analyzer = ctx.analyzer(&node);
        let env = analyzer.env_around(&node);
        let callees = analyzer.callees(&node, &env);
        let Some(callee) = callees.first() else {
            continue;
        };
        let args = arguments(&node);
        let argument_nodes: Vec<SyntaxNode> = list.children().filter(|child| child.kind() == ARGUMENT).collect();
        let level = analyzer.level();
        let params: Vec<&Param> = callee.callable.params_at(level).collect();
        if options.parameter_names {
            parameter_hints(&callees, &params, &args, &argument_nodes, range, &mut out);
        }
        if options.closure_types {
            closure_hints(&analyzer, callee, &params, &args, &env, range, &mut out);
        }
    }
    out.sort_by_key(|hint| hint.offset);
    out
}

fn parameter_hints(
    callees: &[ResolvedCallable],
    params: &[&Param],
    args: &[Arg],
    argument_nodes: &[SyntaxNode],
    range: Option<TextRange>,
    out: &mut Vec<InlayHint>,
) {
    let single = params.len() == 1 && !params[0].variadic;
    for (position, (arg, node)) in args.iter().zip(argument_nodes).enumerate() {
        if arg.name.is_some() || arg.spread {
            break;
        }
        let Some(expr) = &arg.expr else {
            continue;
        };
        let Some(param) = params.get(position) else {
            break;
        };
        if param.variadic {
            break;
        }
        let offset = ast::start(node);
        if range.is_some_and(|range| !range.contains_inclusive(php_syntax::TextSize::from(offset))) {
            continue;
        }
        if param.name.chars().count() < 2 || param.name.starts_with('_') {
            continue;
        }
        // A call that several classes answer shows the name only when all of them agree on it.
        if callees.iter().any(|other| {
            other
                .callable
                .params
                .get(position)
                .is_none_or(|other_param| other_param.name != param.name)
        }) {
            continue;
        }
        if single && !is_unclear_literal(expr) {
            continue;
        }
        if names_of(expr).iter().any(|name| same_name(name, &param.name)) {
            continue;
        }
        out.push(InlayHint {
            offset,
            label: format!("{}:", param.name),
            kind: HintKind::Parameter,
            insert: Some(format!("{}: ", param.name)),
        });
    }
}

/// `true`, `false` and `null` mean nothing without the name of the parameter they are for.
fn is_unclear_literal(expr: &SyntaxNode) -> bool {
    let text = text_of(expr).to_ascii_lowercase();
    matches!(expr.kind(), NAME | LITERAL) && matches!(text.as_str(), "true" | "false" | "null")
}

/// The names an argument goes by: a variable, a property, a call of a method, a constant.
fn names_of(expr: &SyntaxNode) -> Vec<String> {
    let mut names = Vec::new();
    let mut push = |name: String| {
        let lower = name.to_ascii_lowercase();
        for prefix in ["get", "is", "has"] {
            if let Some(rest) = lower.strip_prefix(prefix) {
                if !rest.is_empty() && name.chars().nth(prefix.len()).is_some_and(char::is_uppercase) {
                    names.push(rest.to_string());
                }
            }
        }
        names.push(lower);
    };
    match expr.kind() {
        VARIABLE_EXPR => push(text_of(expr).trim_start_matches('$').to_string()),
        PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR => {
            if let Some(last) = expr.children().last() {
                push(text_of(&last).trim_start_matches('$').to_string());
            }
        }
        CALL_EXPR => {
            if let Some(callee) = expr.children().next() {
                if let Some(last) = callee.children().filter(|child| child.kind() == NAME).last() {
                    push(text_of(&last));
                } else if callee.kind() == NAME {
                    push(text_of(&callee));
                }
            }
        }
        NAME => push(text_of(expr)),
        PAREN_EXPR => {
            if let Some(inner) = expr.children().next() {
                names.extend(names_of(&inner));
            }
        }
        _ => {}
    }
    names
}

/// Names compare without case and without underscores, so `$user_id` is `$userId`.
fn same_name(left: &str, right: &str) -> bool {
    let normalize = |name: &str| {
        name.chars()
            .filter(|c| *c != '_')
            .collect::<String>()
            .to_ascii_lowercase()
    };
    normalize(left) == normalize(right)
}

/// The types a callable parameter promises its closure, for the parameters the closure leaves
/// without one.
fn closure_hints(
    analyzer: &crate::infer::Analyzer<'_>,
    callee: &ResolvedCallable,
    params: &[&Param],
    args: &[Arg],
    env: &crate::infer::Env,
    range: Option<TextRange>,
    out: &mut Vec<InlayHint>,
) {
    let level = analyzer.level();
    let (bound, _) = analyzer.bind_call(callee, args, env);
    for (position, arg) in args.iter().enumerate() {
        let Some(expr) = &arg.expr else {
            continue;
        };
        if !matches!(expr.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR) {
            continue;
        }
        let param = match &arg.name {
            Some(name) => params.iter().find(|param| &param.name == name),
            None => params.get(position),
        };
        let Some(expected) = param.and_then(|param| param.effective_type(level)) else {
            continue;
        };
        let expected = expected.substitute(&bound, callee.receiver.as_ref(), callee.self_name.as_deref());
        let signature = expected.members().iter().find_map(|member| match member {
            Type::Callable(Some(signature)) => Some(signature.clone()),
            _ => None,
        });
        let Some(signature) = signature else {
            continue;
        };
        let Some(list) = child_of(expr, PARAMETER_LIST) else {
            continue;
        };
        for (index, parameter) in list.children().filter(|child| child.kind() == PARAMETER).enumerate() {
            if parameter.children().any(|child| ast::is_type_node(child.kind())) {
                continue;
            }
            let Some(variable) = ast::first_token(&parameter, VARIABLE) else {
                continue;
            };
            let Some(wanted) = signature.params.get(index) else {
                continue;
            };
            let shown = wanted.ty.display(true);
            if matches!(wanted.ty, Type::Mixed | Type::Unknown) || has_template(&wanted.ty) {
                continue;
            }
            let offset = u32::from(variable.text_range().start());
            if range.is_some_and(|range| !range.contains_inclusive(php_syntax::TextSize::from(offset))) {
                continue;
            }
            out.push(InlayHint {
                offset,
                label: shown.clone(),
                kind: HintKind::Type,
                // A class may need an import first, so only types made of built-ins can be written.
                insert: wanted.ty.class_names().is_empty().then(|| format!("{shown} ")),
            });
        }
    }
}

fn has_template(ty: &Type) -> bool {
    let mut names = Vec::new();
    crate::infer::template_names(ty, &mut names);
    !names.is_empty()
}
