//! Calls: the wrong number of arguments, a named argument the callee does not have, and an
//! argument whose type the parameter can never take.

use php_index::{Callable, Origin, Param};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::members::unreliable_receiver;
use super::types::sure_type;
use super::util::member_name;
use super::{Cx, Fix};
use crate::ast::{child_of, text_of, tokens};
use crate::infer::{Arg, ResolvedCallable, arguments};

pub(super) fn run(cx: &Cx) {
    if !(cx.on("wrong-argument-count") || cx.on("undefined-named-argument") || cx.on("argument-type-mismatch")) {
        return;
    }
    for node in &cx.nodes {
        if matches!(node.kind(), CALL_EXPR | NEW_EXPR) {
            check_call(cx, node);
        }
    }
}

enum Problem {
    TooFew { required: usize, given: usize },
    TooMany { max: usize, given: usize },
}

fn check_call(cx: &Cx, call: &SyntaxNode) {
    let Some(list) = child_of(call, ARGUMENT_LIST) else {
        return;
    };
    if tokens(&list).any(|token| token.kind() == ELLIPSIS)
        || list.children().any(|argument| crate::infer::is_placeholder(&argument))
    {
        return;
    }
    let arguments_nodes: Vec<SyntaxNode> = list.children().filter(|child| child.kind() == ARGUMENT).collect();
    let args = arguments(call);
    if args.iter().any(|arg| arg.spread) {
        return;
    }
    if let Some(callee) = call.children().next().filter(|_| call.kind() == CALL_EXPR) {
        if let Some(object) = callee
            .children()
            .next()
            .filter(|_| callee.kind() == PROPERTY_FETCH_EXPR)
        {
            if unreliable_receiver(cx, &callee, &object) {
                return;
            }
        }
        if callee.kind() == SCOPED_ACCESS_EXPR {
            if let Some(qualifier) = callee.children().next().filter(|qualifier| qualifier.kind() != NAME) {
                if unreliable_receiver(cx, &callee, &qualifier) {
                    return;
                }
            }
        }
    }
    let analyzer = cx.file.analyzer(call);
    let env = analyzer.env_around(call);
    let mut callees = analyzer.callees(call, &env);
    callees.dedup_by(|left, right| left.name == right.name);
    let [callee] = callees.as_slice() else {
        return;
    };
    if callee.name == "closure" || callee.via_mixin {
        return;
    }
    let candidates = candidates_of(cx, callee);
    let level = cx.index.level;

    let positional = args.iter().filter(|arg| arg.name.is_none()).count();
    let named: Vec<String> = args.iter().filter_map(|arg| arg.name.clone()).collect();

    if cx.on("undefined-named-argument") {
        for (arg, node) in args.iter().zip(&arguments_nodes) {
            let Some(name) = &arg.name else {
                continue;
            };
            let known = candidates.iter().any(|callable| {
                callable
                    .params_at(level)
                    .any(|param| &param.name == name || param.variadic)
            });
            if !known {
                if let Some(label) = tokens(node).find(|token| !token.kind().is_trivia()) {
                    cx.report(
                        "undefined-named-argument",
                        label.text_range(),
                        format!("{} has no parameter named '{name}'", describe(callee)),
                        Fix::None,
                    );
                }
            }
        }
    }
    if cx.on("wrong-argument-count") && !class_may_differ(call) && !is_abstract_callee(cx, callee) {
        let problems: Vec<Option<Problem>> = candidates
            .iter()
            .map(|callable| arity(cx, callable, positional, &named))
            .collect();
        if !problems.is_empty() && problems.iter().all(Option::is_some) {
            if let Some(Some(problem)) = problems.into_iter().next() {
                report_arity(cx, call, &arguments_nodes, callee, &problem);
            }
        }
    }
    if cx.on("argument-type-mismatch") {
        check_types(cx, &analyzer, &env, callee, &candidates, &args);
    }
}

fn describe(callee: &ResolvedCallable) -> String {
    if callee.is_constructor {
        format!(
            "Constructor of {}",
            callee.constructed.as_deref().unwrap_or(&callee.name)
        )
    } else {
        format!("'{}'", callee.name)
    }
}

/// Every declaration the call may resolve to: a function the standard library declares once for
/// each way of calling it has several.
fn candidates_of(cx: &Cx, callee: &ResolvedCallable) -> Vec<Callable> {
    if !callee.name.contains("::") && !callee.is_constructor {
        let overloads = cx.index.function_overloads(&callee.name);
        if !overloads.is_empty() {
            return overloads
                .iter()
                .map(|function| function.decl.callable.clone())
                .collect();
        }
    }
    vec![callee.callable.clone()]
}

fn arity(cx: &Cx, callable: &Callable, positional: usize, named: &[String]) -> Option<Problem> {
    let level = cx.index.level;
    let params: Vec<&Param> = callable.params_at(level).collect();
    let variadic = params.iter().any(|param| param.variadic);
    if positional > params.len() && !variadic && named.is_empty() && !callable.reads_all_arguments {
        return Some(Problem::TooMany {
            max: params.len(),
            given: positional,
        });
    }
    let required: Vec<(usize, &&Param)> = params
        .iter()
        .enumerate()
        .filter(|(_, param)| !param.is_optional())
        .collect();
    let missing = required
        .iter()
        .any(|(position, param)| *position >= positional && !named.contains(&param.name));
    if missing {
        return Some(Problem::TooFew {
            required: required.len(),
            given: positional + named.len(),
        });
    }
    None
}

fn report_arity(cx: &Cx, call: &SyntaxNode, arguments: &[SyntaxNode], callee: &ResolvedCallable, problem: &Problem) {
    let label = describe(callee);
    match problem {
        Problem::TooFew { required, given } => cx.report(
            "wrong-argument-count",
            callee_range(call),
            format!("{label} expects at least {required} argument(s), {given} given"),
            Fix::None,
        ),
        Problem::TooMany { max, given } => {
            let (Some(first), Some(last)) = (arguments.get(*max), arguments.last()) else {
                return;
            };
            let range = TextRange::new(first.text_range().start(), last.text_range().end());
            cx.report(
                "wrong-argument-count",
                range,
                format!("{label} takes at most {max} argument(s), {given} given"),
                Fix::None,
            );
        }
    }
}

/// `new static`, `new $class`: whichever class that turns out to be takes its own arguments.
fn class_may_differ(call: &SyntaxNode) -> bool {
    if call.kind() != NEW_EXPR {
        return false;
    }
    match call.children().next() {
        Some(class) if class.kind() == NAME => text_of(&class).eq_ignore_ascii_case("static"),
        Some(class) => class.kind() != ANONYMOUS_CLASS,
        None => true,
    }
}

/// A method that something below it may declare with more parameters.
fn is_abstract_callee(cx: &Cx, callee: &ResolvedCallable) -> bool {
    let Some((class, method)) = callee.name.split_once("::") else {
        return false;
    };
    let ty = php_index::Type::class(class.to_string());
    cx.index
        .find_method(&ty, method)
        .is_some_and(|found| found.member.is_abstract || found.class.decl.kind == php_index::ClassKind::Interface)
}

/// The part of a call that names what is called.
fn callee_range(call: &SyntaxNode) -> TextRange {
    let name = match call.kind() {
        NEW_EXPR => child_of(call, NAME),
        _ => call.children().next().and_then(|callee| match callee.kind() {
            NAME => Some(callee),
            PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR => member_name(&callee),
            _ => None,
        }),
    };
    name.map_or_else(|| call.text_range(), |name| name.text_range())
}

fn check_types(
    cx: &Cx,
    analyzer: &crate::infer::Analyzer<'_>,
    env: &crate::infer::Env,
    callee: &ResolvedCallable,
    candidates: &[Callable],
    args: &[Arg],
) {
    let [callable] = candidates else {
        return;
    };
    let level = cx.index.level;
    let params: Vec<&Param> = callable.params_at(level).collect();
    let internal = is_internal(cx, callee);
    for (position, arg) in args.iter().enumerate() {
        let Some(expr) = &arg.expr else {
            continue;
        };
        let param = match &arg.name {
            Some(name) => params.iter().find(|param| &param.name == name),
            None => params
                .get(position)
                .or_else(|| params.last().filter(|param| param.variadic)),
        };
        let Some(param) = param else {
            continue;
        };
        let Some(wanted) = param.native_type(level) else {
            continue;
        };
        // A default of `null` makes the type nullable, as `array $mapping = null` is.
        let wanted = if param
            .default
            .as_deref()
            .is_some_and(|default| default.trim().eq_ignore_ascii_case("null"))
        {
            php_index::Type::union([wanted.clone(), php_index::Type::Null])
        } else {
            wanted.clone()
        };
        let Some(given) = sure_type(cx, analyzer, env, expr) else {
            continue;
        };
        if cx.type_mismatch(&given, &wanted, internal) {
            cx.report(
                "argument-type-mismatch",
                expr.text_range(),
                format!(
                    "Argument of type '{}' cannot be passed to parameter '${}' of type '{}'",
                    given.display(true),
                    param.name,
                    wanted.display(true)
                ),
                Fix::None,
            );
        }
    }
}

/// Whether the callee is part of the standard library.
fn is_internal(cx: &Cx, callee: &ResolvedCallable) -> bool {
    match callee.name.split_once("::") {
        Some((class, _)) => cx
            .index
            .class(class)
            .is_some_and(|found| found.file.origin == Origin::Stub),
        None => cx
            .index
            .function(&callee.name)
            .is_some_and(|found| found.file.origin == Origin::Stub),
    }
}
