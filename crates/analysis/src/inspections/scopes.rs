//! Variables: what is read and never assigned, what is assigned and never read, and the
//! parameters a private or final method never uses. One scan per function body answers all of it.

use std::collections::HashSet;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange, WalkEvent};

use super::util::{has_modifier, in_guard, is_bare_variable};
use super::{Cx, Fix};
use crate::ast::{self, child_of, first_token, has_token, text_of, tokens};
use crate::refs::is_write_target;

const SUPERGLOBALS: &[&str] = &[
    "GLOBALS",
    "_SERVER",
    "_GET",
    "_POST",
    "_FILES",
    "_COOKIE",
    "_SESSION",
    "_REQUEST",
    "_ENV",
    "http_response_header",
    "php_errormsg",
    "this",
];

/// A plain `$name = value` that may turn out to be unused.
struct Assign {
    name: String,
    range: TextRange,
    node: SyntaxNode,
}

#[derive(Default)]
struct Scan {
    bound: HashSet<String>,
    reads: HashSet<String>,
    /// Reads to check against what the scope binds, with the range to report.
    free_reads: Vec<(String, TextRange)>,
    assigns: Vec<Assign>,
    /// Names that something asks about with `isset`, `empty` or `??`, which are not meant to be there.
    asked: HashSet<String>,
    /// Names that something else may read or write: by reference, global, static.
    escapes: HashSet<String>,
    dynamic: bool,
}

pub(super) fn run(cx: &Cx) {
    let wants_variables = cx.on("undefined-variable") || cx.on("unused-variable") || cx.on("unused-parameter");
    for node in &cx.nodes {
        match node.kind() {
            FUNCTION_DECLARATION | METHOD_DECLARATION | CLOSURE_EXPR if wants_variables => check_scope(cx, node),
            VARIABLE_EXPR if cx.on("this-in-static-context") => check_this(cx, node),
            _ => {}
        }
    }
}

fn check_scope(cx: &Cx, scope: &SyntaxNode) {
    let Some(body) = child_of(scope, BLOCK) else {
        return;
    };
    let mut scan = Scan::default();
    let mut params: Vec<(String, TextRange, bool)> = Vec::new();
    if let Some(list) = child_of(scope, PARAMETER_LIST) {
        for parameter in list.children().filter(|child| child.kind() == PARAMETER) {
            if let Some(token) = first_token(&parameter, VARIABLE) {
                let name = token.text().trim_start_matches('$').to_string();
                let by_ref = has_token(&parameter, AMP);
                scan.bound.insert(name.clone());
                params.push((name, token.text_range(), by_ref));
            }
        }
    }
    if scope.kind() == CLOSURE_EXPR {
        if let Some(uses) = child_of(scope, CLOSURE_USE) {
            for variable in uses.children().filter(|child| child.kind() == CLOSURE_USE_VARIABLE) {
                if let Some(token) = first_token(&variable, VARIABLE) {
                    let name = token.text().trim_start_matches('$').to_string();
                    scan.bound.insert(name.clone());
                    scan.reads.insert(name.clone());
                    if has_token(&variable, AMP) {
                        scan.escapes.insert(name);
                    }
                }
            }
        }
    }
    scan_body(cx, &body, &mut scan);
    if cx.in_error(scope.text_range()) {
        return;
    }
    if scan.dynamic {
        return;
    }
    if cx.on("undefined-variable") {
        for (name, range) in &scan.free_reads {
            if scan.bound.contains(name) || scan.asked.contains(name) || SUPERGLOBALS.contains(&name.as_str()) {
                continue;
            }
            cx.report(
                "undefined-variable",
                *range,
                format!("Undefined variable '${name}'"),
                Fix::None,
            );
        }
    }
    if cx.on("unused-variable") {
        for assign in &scan.assigns {
            let name = &assign.name;
            if scan.reads.contains(name) || scan.escapes.contains(name) || name.starts_with('_') {
                continue;
            }
            report_unused_assignment(cx, assign);
        }
    }
    if cx.on("unused-parameter") && scope.kind() == METHOD_DECLARATION {
        check_unused_parameters(cx, scope, &body, &params, &scan);
    }
}

fn report_unused_assignment(cx: &Cx, assign: &Assign) {
    let name = &assign.name;
    let fix = assignment_fix(&assign.node);
    cx.report(
        "unused-variable",
        assign.range,
        format!("Variable '${name}' is assigned but never used"),
        fix,
    );
}

/// How an unused assignment can be removed, when the assignment is a statement of its own.
fn assignment_fix(assign: &SyntaxNode) -> Fix {
    let Some(statement) = assign.parent().filter(|parent| parent.kind() == EXPR_STATEMENT) else {
        return Fix::None;
    };
    let Some(value) = assign.children().last() else {
        return Fix::None;
    };
    Fix::RemoveAssignment {
        statement: statement.text_range(),
        prefix: TextRange::new(assign.text_range().start(), value.text_range().start()),
        pure: is_pure(&value),
    }
}

/// An expression whose evaluation does nothing but produce a value.
pub(super) fn is_pure(node: &SyntaxNode) -> bool {
    match node.kind() {
        LITERAL | VARIABLE_EXPR | NAME => true,
        PAREN_EXPR | PREFIX_EXPR | CAST_EXPR | BINARY_EXPR | TERNARY_EXPR => {
            node.children().all(|child| is_pure(&child))
        }
        ARRAY_EXPR | ARRAY_ITEM => node.children().all(|child| is_pure(&child)),
        SCOPED_ACCESS_EXPR => node.children().all(|child| child.kind() == NAME),
        _ => false,
    }
}

fn scan_body(cx: &Cx, body: &SyntaxNode, scan: &mut Scan) {
    let mut preorder = body.preorder_with_tokens();
    while let Some(event) = preorder.next() {
        let WalkEvent::Enter(element) = event else {
            continue;
        };
        let php_syntax::SyntaxElement::Node(node) = element else {
            continue;
        };
        match node.kind() {
            FUNCTION_DECLARATION
            | METHOD_DECLARATION
            | CLASS_DECLARATION
            | INTERFACE_DECLARATION
            | TRAIT_DECLARATION
            | ENUM_DECLARATION => preorder.skip_subtree(),
            ANONYMOUS_CLASS => {
                if let Some(arguments) = child_of(&node, ARGUMENT_LIST) {
                    scan_arguments(cx, &arguments, scan);
                }
                preorder.skip_subtree();
            }
            CLOSURE_EXPR => {
                scan_closure(&node, scan);
                preorder.skip_subtree();
            }
            ARROW_FUNCTION_EXPR => {
                scan_arrow(cx, &node, scan);
                preorder.skip_subtree();
            }
            VARIABLE_VARIABLE | INCLUDE_EXPR | EVAL_EXPR => scan.dynamic = true,
            DOLLAR_BRACE_INTERPOLATION => match child_of(&node, NAME) {
                Some(name) => {
                    scan.reads.insert(text_of(&name));
                }
                None => scan.dynamic = true,
            },
            CALL_EXPR => scan_call(cx, &node, scan),
            VARIABLE_EXPR => scan_variable(cx, &node, scan),
            STATIC_VARIABLE | CATCH_CLAUSE | GLOBAL_STATEMENT => {
                for token in tokens(&node).filter(|token| token.kind() == VARIABLE) {
                    let name = token.text().trim_start_matches('$').to_string();
                    scan.bound.insert(name.clone());
                    if node.kind() != CATCH_CLAUSE {
                        scan.escapes.insert(name);
                    }
                }
            }
            _ => {}
        }
    }
}

fn scan_arguments(cx: &Cx, arguments: &SyntaxNode, scan: &mut Scan) {
    for node in arguments.descendants() {
        if node.kind() == VARIABLE_EXPR {
            scan_variable(cx, &node, scan);
        }
    }
}

fn scan_closure(closure: &SyntaxNode, scan: &mut Scan) {
    let Some(uses) = child_of(closure, CLOSURE_USE) else {
        return;
    };
    for variable in uses.children().filter(|child| child.kind() == CLOSURE_USE_VARIABLE) {
        let Some(token) = first_token(&variable, VARIABLE) else {
            continue;
        };
        let name = token.text().trim_start_matches('$').to_string();
        scan.reads.insert(name.clone());
        if has_token(&variable, AMP) {
            scan.escapes.insert(name.clone());
            scan.bound.insert(name);
        } else {
            scan.free_reads.push((name, token.text_range()));
        }
    }
}

/// An arrow function sees the variables around it: what it reads and does not bind itself is read
/// from the scope it is in.
fn scan_arrow(cx: &Cx, arrow: &SyntaxNode, scan: &mut Scan) {
    let mut inner = Scan::default();
    if let Some(list) = child_of(arrow, PARAMETER_LIST) {
        for parameter in list.children().filter(|child| child.kind() == PARAMETER) {
            if let Some(token) = first_token(&parameter, VARIABLE) {
                inner.bound.insert(token.text().trim_start_matches('$').to_string());
            }
        }
    }
    for child in arrow
        .children()
        .filter(|child| !matches!(child.kind(), PARAMETER_LIST | RETURN_TYPE))
    {
        scan_body(cx, &child, &mut inner);
        if child.kind() == VARIABLE_EXPR {
            scan_variable(cx, &child, &mut inner);
        }
    }
    scan.dynamic |= inner.dynamic;
    scan.asked.extend(inner.asked);
    for name in inner.reads {
        if !inner.bound.contains(&name) {
            scan.reads.insert(name);
        }
    }
    for (name, range) in inner.free_reads {
        if !inner.bound.contains(&name) {
            scan.free_reads.push((name, range));
        }
    }
    scan.escapes.extend(inner.escapes);
}

fn scan_call(cx: &Cx, call: &SyntaxNode, scan: &mut Scan) {
    let Some(callee) = call.children().next() else {
        return;
    };
    if callee.kind() == NAME {
        match text_of(&callee).trim_start_matches('\\').to_ascii_lowercase().as_str() {
            "compact" => {
                let Some(arguments) = child_of(call, ARGUMENT_LIST) else {
                    return;
                };
                for argument in arguments.children().filter(|child| child.kind() == ARGUMENT) {
                    match argument
                        .children()
                        .next()
                        .and_then(|value| crate::infer::literal_string(&value))
                    {
                        Some(name) => {
                            scan.reads.insert(name);
                        }
                        None => scan.dynamic = true,
                    }
                }
                return;
            }
            "extract" | "get_defined_vars" | "parse_str" | "func_get_args" | "func_get_arg" | "func_num_args" => {
                scan.dynamic = true;
                return;
            }
            _ => {}
        }
    }
    bind_reference_arguments(cx, call, scan);
}

/// A variable passed to a parameter taken by reference is written by the call, and one passed to
/// a function nothing is known of may be.
fn bind_reference_arguments(cx: &Cx, call: &SyntaxNode, scan: &mut Scan) {
    let Some(arguments) = child_of(call, ARGUMENT_LIST) else {
        return;
    };
    let bare: Vec<(usize, SyntaxNode)> = arguments
        .children()
        .filter(|child| child.kind() == ARGUMENT)
        .enumerate()
        .filter_map(|(position, argument)| {
            let value = argument.children().last()?;
            is_bare_variable(&value).then_some((position, value))
        })
        .collect();
    if bare.is_empty() {
        return;
    }
    let analyzer = cx.file.analyzer(call);
    let env = analyzer.env_around(call);
    let callees = analyzer.callees(call, &env);
    let level = cx.index.level;
    for (position, value) in bare {
        let by_ref = callees.is_empty()
            || callees.iter().any(|callee| {
                let params: Vec<_> = callee.callable.params_at(level).collect();
                params
                    .get(position)
                    .or_else(|| params.last().filter(|param| param.variadic))
                    .is_some_and(|param| param.by_ref)
            });
        if by_ref {
            let name = text_of(&value).trim_start_matches('$').to_string();
            scan.bound.insert(name.clone());
            scan.escapes.insert(name);
        }
    }
}

fn scan_variable(_cx: &Cx, node: &SyntaxNode, scan: &mut Scan) {
    let name = text_of(node).trim_start_matches('$').to_string();
    if name == "this" {
        return;
    }
    if is_property_name(node) {
        return;
    }
    let parent_kind = node.parent().map(|parent| parent.kind());
    if is_write_target(node) {
        scan.bound.insert(name.clone());
        let plain = node.parent().is_some_and(|parent| {
            parent.kind() == ASSIGN_EXPR
                && parent
                    .parent()
                    .is_some_and(|statement| statement.kind() == EXPR_STATEMENT)
                && parent.children().next().as_ref() == Some(node)
                && has_token(&parent, ASSIGN)
                && !tokens(&parent).any(|token| token.kind() == AMP)
        });
        if plain {
            if let Some(assign) = node.parent() {
                scan.assigns.push(Assign {
                    name,
                    range: node.text_range(),
                    node: assign,
                });
            }
        } else {
            // Writing is also a use of what was there: a compound assignment, an element, a loop's variables.
            scan.reads.insert(name.clone());
            if parent_kind == Some(GLOBAL_STATEMENT) {
                scan.escapes.insert(name);
            }
        }
        return;
    }
    scan.reads.insert(name.clone());
    if in_guard(node) {
        scan.asked.insert(name);
    } else {
        scan.free_reads.push((name, node.text_range()));
    }
}

fn is_property_name(node: &SyntaxNode) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == STATIC_PROPERTY_EXPR && parent.children().next().as_ref() != Some(node))
}

fn check_unused_parameters(
    cx: &Cx,
    method: &SyntaxNode,
    body: &SyntaxNode,
    params: &[(String, TextRange, bool)],
    scan: &Scan,
) {
    let restricted = has_modifier(method, PRIVATE_KW) || has_modifier(method, FINAL_KW);
    if !restricted || child_of(method, ATTRIBUTE_LIST).is_some() {
        return;
    }
    if !body.children().any(|child| !matches!(child.kind(), EMPTY_STATEMENT)) {
        return;
    }
    let Some(name) = child_of(method, NAME).map(|name| text_of(&name)) else {
        return;
    };
    if name.eq_ignore_ascii_case("__construct") || name.starts_with("__") {
        return;
    }
    if let Some(class_node) = method.parent().and_then(|body| body.parent()) {
        let usage = super::unused::usage_of(cx, &class_node);
        if usage.dynamic || usage.is_callback(&name) {
            return;
        }
    }
    let analyzer = cx.file.analyzer(method);
    if let Some(class) = &analyzer.class {
        let ty = php_index::Type::class(class.name.clone());
        let overrides = cx
            .index
            .ancestors(&ty)
            .iter()
            .skip(1)
            .any(|ancestor| ancestor.class.decl.method(&name).is_some());
        if overrides || class.anonymous {
            return;
        }
    }
    for (param, range, by_ref) in params {
        let assigned = scan.assigns.iter().any(|assign| &assign.name == param);
        if *by_ref || param.starts_with('_') || scan.reads.contains(param) || scan.escapes.contains(param) || assigned {
            continue;
        }
        cx.report(
            "unused-parameter",
            *range,
            format!("Parameter '${param}' is never used"),
            Fix::None,
        );
    }
}

/// `$this` in a static method or a static closure, where there is no object.
fn check_this(cx: &Cx, node: &SyntaxNode) {
    if text_of(node) != "$this" || is_property_name(node) {
        return;
    }
    let mut inside_closure = false;
    for ancestor in node.ancestors() {
        match ancestor.kind() {
            METHOD_DECLARATION => {
                if has_modifier(&ancestor, STATIC_KW) && !inside_closure {
                    report_this(cx, node);
                }
                return;
            }
            CLOSURE_EXPR | ARROW_FUNCTION_EXPR => {
                if tokens(&ancestor).any(|token| token.kind() == STATIC_KW) {
                    report_this(cx, node);
                    return;
                }
                // A closure made in a static method may be bound to an object afterwards.
                inside_closure = true;
            }
            FUNCTION_DECLARATION => {
                report_this(cx, node);
                return;
            }
            kind if ast::is_class_like(kind) => return,
            _ => {}
        }
    }
}

fn report_this(cx: &Cx, node: &SyntaxNode) {
    cx.report(
        "this-in-static-context",
        node.text_range(),
        "'$this' cannot be used where there is no object".to_string(),
        Fix::None,
    );
}
