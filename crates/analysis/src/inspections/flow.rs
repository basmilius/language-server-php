//! Control flow: code that cannot run, a function that can end without the value it promises,
//! values the declared return type cannot take, and conditions that are decided before they run.

use php_index::{Callable, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode, TextRange};

use super::types::{Kinds, kinds_of, sure_type};
use super::{Cx, Fix};
use crate::ast::{self, child_of, text_of, tokens};
use crate::infer::Analyzer;

/// How a statement ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// Control goes on to what follows.
    Completes,
    /// The function does not go on: `return`, `throw`, `exit`.
    Terminates,
    /// `break` or `continue`: control leaves the loop or `switch` around.
    Jumps,
    /// Not known.
    Unknown,
}

pub(super) fn run(cx: &Cx) {
    for node in &cx.nodes {
        match node.kind() {
            FUNCTION_DECLARATION | METHOD_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR => check_function(cx, node),
            BLOCK | STATEMENT_LIST | CASE_CLAUSE | DEFAULT_CLAUSE | SOURCE_FILE => check_unreachable(cx, node),
            IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT | DO_WHILE_STATEMENT | TERNARY_EXPR => {
                check_condition(cx, node)
            }
            BINARY_EXPR => check_comparison(cx, node),
            _ => {}
        }
    }
}

// Unreachable code ------------------------------------------------------------------------------

fn statements_of(container: &SyntaxNode) -> Vec<SyntaxNode> {
    if !matches!(container.kind(), CASE_CLAUSE | DEFAULT_CLAUSE) {
        return container.children().collect();
    }
    let mut after_colon = false;
    let mut out = Vec::new();
    for element in container.children_with_tokens() {
        match element {
            php_syntax::SyntaxElement::Token(token) if matches!(token.kind(), COLON | SEMICOLON) => {
                after_colon = true;
            }
            php_syntax::SyntaxElement::Node(node) if after_colon => out.push(node),
            _ => {}
        }
    }
    out
}

/// A statement that exists whether or not control reaches it.
fn is_hoisted(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        FUNCTION_DECLARATION
            | CLASS_DECLARATION
            | INTERFACE_DECLARATION
            | TRAIT_DECLARATION
            | ENUM_DECLARATION
            | USE_STATEMENT
            | CONST_STATEMENT
            | NAMESPACE_DECLARATION
            | EMPTY_STATEMENT
            | HALT_COMPILER_STATEMENT
            | LABEL_STATEMENT
            | DECLARE_STATEMENT
            | ERROR
    )
}

fn check_unreachable(cx: &Cx, container: &SyntaxNode) {
    if !cx.on("unreachable-code") || has_goto(container) {
        return;
    }
    let statements = statements_of(container);
    let Some(position) = statements.iter().position(|statement| {
        !is_hoisted(statement.kind())
            && statement_end(cx, statement) != End::Completes
            && statement_end(cx, statement) != End::Unknown
    }) else {
        return;
    };
    let rest: Vec<&SyntaxNode> = statements[position + 1..]
        .iter()
        .filter(|statement| !is_hoisted(statement.kind()))
        .collect();
    let (Some(first), Some(last)) = (rest.first(), rest.last()) else {
        return;
    };
    let range = TextRange::new(first.text_range().start(), last.text_range().end());
    cx.report(
        "unreachable-code",
        range,
        "Unreachable code".to_string(),
        Fix::RemoveRange { range },
    );
}

fn has_goto(node: &SyntaxNode) -> bool {
    node.descendants()
        .any(|descendant| matches!(descendant.kind(), GOTO_STATEMENT | LABEL_STATEMENT))
}

// How statements end ----------------------------------------------------------------------------

/// Whether control can reach the end of a body, as far as it is certain.
pub(crate) fn body_completes(cx: &Cx, body: &SyntaxNode) -> bool {
    sequence_end(cx, &body.children().collect::<Vec<_>>()) == End::Completes
}

/// Whether control can reach the end of a run of statements, as far as it is certain.
pub(crate) fn statements_complete(cx: &Cx, statements: &[SyntaxNode]) -> bool {
    sequence_end(cx, statements) == End::Completes
}

/// Whether control certainly leaves a run of statements without reaching its end.
pub(crate) fn statements_leave(cx: &Cx, statements: &[SyntaxNode]) -> bool {
    matches!(sequence_end(cx, statements), End::Terminates)
}

fn sequence_end(cx: &Cx, statements: &[SyntaxNode]) -> End {
    let mut unknown = false;
    for statement in statements {
        if is_hoisted(statement.kind()) {
            continue;
        }
        match statement_end(cx, statement) {
            End::Completes => {}
            End::Unknown => unknown = true,
            other => return if unknown { End::Unknown } else { other },
        }
    }
    if unknown { End::Unknown } else { End::Completes }
}

fn branch_end(cx: &Cx, branch: &SyntaxNode) -> End {
    match branch.kind() {
        BLOCK | STATEMENT_LIST => sequence_end(cx, &branch.children().collect::<Vec<_>>()),
        _ => statement_end(cx, branch),
    }
}

/// How a set of alternatives ends together: one that goes on makes the whole go on.
fn alternatives_end(ends: &[End]) -> End {
    if ends.contains(&End::Completes) {
        return End::Completes;
    }
    if ends.contains(&End::Unknown) {
        return End::Unknown;
    }
    if ends.iter().all(|end| *end == End::Terminates) {
        End::Terminates
    } else {
        End::Jumps
    }
}

fn statement_end(cx: &Cx, statement: &SyntaxNode) -> End {
    match statement.kind() {
        RETURN_STATEMENT => End::Terminates,
        BREAK_STATEMENT | CONTINUE_STATEMENT => End::Jumps,
        GOTO_STATEMENT => End::Unknown,
        BLOCK => branch_end(cx, statement),
        EXPR_STATEMENT => expression_end(cx, statement),
        IF_STATEMENT => if_end(cx, statement),
        WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT => loop_end(statement),
        SWITCH_STATEMENT => switch_end(cx, statement),
        TRY_STATEMENT => try_end(cx, statement),
        DECLARE_STATEMENT => child_of(statement, BLOCK).map_or(End::Completes, |block| branch_end(cx, &block)),
        ERROR => End::Unknown,
        _ => End::Completes,
    }
}

fn expression_end(cx: &Cx, statement: &SyntaxNode) -> End {
    let Some(expression) = statement.children().next() else {
        return End::Completes;
    };
    match expression.kind() {
        THROW_EXPR | EXIT_EXPR => End::Terminates,
        CALL_EXPR if returns_never(cx, &expression) => End::Terminates,
        _ => End::Completes,
    }
}

fn returns_never(cx: &Cx, call: &SyntaxNode) -> bool {
    let analyzer = cx.file.analyzer(call);
    let env = analyzer.env_around(call);
    let mut callees = analyzer.callees(call, &env);
    callees.dedup_by(|left, right| left.name == right.name);
    let [callee] = callees.as_slice() else {
        return false;
    };
    callee.callable.native_return(cx.index.level) == Some(&Type::Never)
}

fn if_end(cx: &Cx, statement: &SyntaxNode) -> End {
    let mut parts = statement.children();
    let condition = parts.next();
    if condition.as_ref().is_some_and(is_constant_true) {
        return End::Unknown;
    }
    let mut ends = Vec::new();
    let mut has_else = false;
    for part in parts {
        match part.kind() {
            ELSEIF_CLAUSE => {
                let body = part.children().last();
                ends.push(body.map_or(End::Unknown, |body| branch_end(cx, &body)));
            }
            ELSE_CLAUSE => {
                has_else = true;
                let body = part.children().next();
                ends.push(body.map_or(End::Unknown, |body| branch_end(cx, &body)));
            }
            _ => ends.push(branch_end(cx, &part)),
        }
    }
    if !has_else {
        return End::Completes;
    }
    alternatives_end(&ends)
}

fn is_constant_true(condition: &SyntaxNode) -> bool {
    let text = condition.text().to_string();
    let text = text.trim();
    text.eq_ignore_ascii_case("true") || text == "1"
}

fn loop_end(statement: &SyntaxNode) -> End {
    let condition = match statement.kind() {
        WHILE_STATEMENT => statement.children().next(),
        DO_WHILE_STATEMENT => statement.children().last(),
        _ => None,
    };
    let endless = match statement.kind() {
        FOR_STATEMENT => for_condition_is_empty(statement),
        _ => condition.as_ref().is_some_and(is_constant_true),
    };
    if !endless {
        return End::Completes;
    }
    if leaves_loop(statement) {
        End::Completes
    } else {
        End::Terminates
    }
}

/// `for (init; ; step)`: nothing between the two semicolons.
fn for_condition_is_empty(statement: &SyntaxNode) -> bool {
    let mut semicolons = 0;
    for element in statement.children_with_tokens() {
        if element.kind() == SEMICOLON {
            semicolons += 1;
            if semicolons == 2 {
                return true;
            }
        } else if semicolons == 1 && !element.kind().is_trivia() {
            return false;
        }
    }
    false
}

/// Whether a `break` or a `goto` in a loop's body can leave it.
fn leaves_loop(statement: &SyntaxNode) -> bool {
    let mut preorder = statement.preorder();
    let mut depth = 0u32;
    while let Some(event) = preorder.next() {
        match event {
            php_syntax::WalkEvent::Enter(node) => {
                if node == *statement {
                    continue;
                }
                match node.kind() {
                    FUNCTION_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | CLASS_DECLARATION | ANONYMOUS_CLASS => {
                        preorder.skip_subtree();
                    }
                    WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT | SWITCH_STATEMENT => {
                        depth += 1
                    }
                    GOTO_STATEMENT => return true,
                    BREAK_STATEMENT => {
                        let level: u32 = ast::tokens(&node)
                            .find(|token| token.kind() == INT_LITERAL)
                            .or_else(|| {
                                node.children()
                                    .next()
                                    .and_then(|child| ast::first_token(&child, INT_LITERAL))
                            })
                            .and_then(|token| token.text().parse().ok())
                            .unwrap_or(1);
                        if level > depth {
                            return true;
                        }
                    }
                    _ => {}
                }
            }
            php_syntax::WalkEvent::Leave(node) => {
                if matches!(
                    node.kind(),
                    WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT | SWITCH_STATEMENT
                ) && node != *statement
                {
                    depth = depth.saturating_sub(1);
                }
            }
        }
    }
    false
}

fn switch_end(cx: &Cx, statement: &SyntaxNode) -> End {
    let clauses: Vec<SyntaxNode> = statement
        .children()
        .filter(|child| matches!(child.kind(), CASE_CLAUSE | DEFAULT_CLAUSE))
        .collect();
    if !clauses.iter().any(|clause| clause.kind() == DEFAULT_CLAUSE) {
        return End::Completes;
    }
    let mut unknown = false;
    for (position, clause) in clauses.iter().enumerate() {
        let last = position + 1 == clauses.len();
        match sequence_end(cx, &statements_of(clause)) {
            End::Jumps => return End::Completes,
            End::Completes if last => return End::Completes,
            End::Unknown => unknown = true,
            _ => {}
        }
    }
    if unknown { End::Unknown } else { End::Terminates }
}

fn try_end(cx: &Cx, statement: &SyntaxNode) -> End {
    if let Some(finally) = child_of(statement, FINALLY_CLAUSE) {
        let ends = finally
            .children()
            .next()
            .map_or(End::Completes, |body| branch_end(cx, &body));
        if ends != End::Completes {
            return ends;
        }
    }
    let mut ends = Vec::new();
    if let Some(body) = statement.children().find(|child| child.kind() == BLOCK) {
        ends.push(branch_end(cx, &body));
    }
    for catch in statement.children().filter(|child| child.kind() == CATCH_CLAUSE) {
        let body = catch.children().find(|child| child.kind() == BLOCK);
        ends.push(body.map_or(End::Unknown, |body| branch_end(cx, &body)));
    }
    alternatives_end(&ends)
}

// Functions -------------------------------------------------------------------------------------

fn check_function(cx: &Cx, function: &SyntaxNode) {
    let wants_missing = cx.on("missing-return");
    let wants_returns = cx.on("return-type-mismatch");
    if !(wants_missing || wants_returns) {
        return;
    }
    let Some(return_type) = child_of(function, RETURN_TYPE) else {
        return;
    };
    let body = child_of(function, BLOCK);
    if body.as_ref().is_some_and(|body| contains_yield(body) || has_goto(body)) {
        return;
    }
    let analyzer = cx.file.analyzer(function);
    let callable = super::types::callable_of(&analyzer, function);
    let Some(declared) = callable.native_return(cx.index.level).cloned() else {
        return;
    };
    let label = declared.display(true);
    if wants_missing && !matches!(declared, Type::Void | Type::Never) {
        if let Some(body) = &body {
            if sequence_end(cx, &body.children().collect::<Vec<_>>()) == End::Completes {
                let anchor =
                    child_of(function, NAME).map_or_else(|| return_type.text_range(), |name| name.text_range());
                cx.report(
                    "missing-return",
                    anchor,
                    format!("This function must return a value of type '{label}', but it can end without returning"),
                    Fix::None,
                );
            }
        }
    }
    if wants_returns {
        check_returns(cx, function, &analyzer, &callable, &declared);
    }
}

fn contains_yield(body: &SyntaxNode) -> bool {
    let mut preorder = body.preorder();
    while let Some(event) = preorder.next() {
        let php_syntax::WalkEvent::Enter(node) = event else {
            continue;
        };
        match node.kind() {
            YIELD_EXPR | YIELD_FROM_EXPR => return true,
            FUNCTION_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | ANONYMOUS_CLASS | CLASS_DECLARATION => {
                preorder.skip_subtree();
            }
            _ => {}
        }
    }
    false
}

fn check_returns(cx: &Cx, function: &SyntaxNode, analyzer: &Analyzer<'_>, _callable: &Callable, declared: &Type) {
    if function.kind() == ARROW_FUNCTION_EXPR {
        if let Some(value) = function
            .children()
            .last()
            .filter(|child| !matches!(child.kind(), PARAMETER_LIST | RETURN_TYPE))
        {
            check_returned_value(cx, analyzer, &value, declared);
        }
        return;
    }
    let Some(body) = child_of(function, BLOCK) else {
        return;
    };
    let mut preorder = body.preorder();
    while let Some(event) = preorder.next() {
        let php_syntax::WalkEvent::Enter(node) = event else {
            continue;
        };
        match node.kind() {
            FUNCTION_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | ANONYMOUS_CLASS | CLASS_DECLARATION => {
                preorder.skip_subtree();
            }
            RETURN_STATEMENT => {
                let value = node.children().next();
                match value {
                    Some(value) => {
                        if matches!(declared, Type::Void) {
                            cx.report(
                                "return-type-mismatch",
                                value.text_range(),
                                "A void function must not return a value".to_string(),
                                Fix::None,
                            );
                        } else if matches!(declared, Type::Never) {
                            cx.report(
                                "return-type-mismatch",
                                node.text_range(),
                                "A never-returning function must not return".to_string(),
                                Fix::None,
                            );
                        } else {
                            check_returned_value(cx, analyzer, &value, declared);
                        }
                    }
                    None => {
                        if !matches!(declared, Type::Void | Type::Never) {
                            cx.report(
                                "return-type-mismatch",
                                node.text_range(),
                                format!("This function must return a value of type '{}'", declared.display(true)),
                                Fix::None,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_returned_value(cx: &Cx, analyzer: &Analyzer<'_>, value: &SyntaxNode, declared: &Type) {
    let env = analyzer.env_around(value);
    let Some(given) = sure_type(cx, analyzer, &env, value) else {
        return;
    };
    if cx.type_mismatch(&given, declared, false) {
        cx.report(
            "return-type-mismatch",
            value.text_range(),
            format!(
                "A value of type '{}' cannot be returned where '{}' is declared",
                given.display(true),
                declared.display(true)
            ),
            Fix::None,
        );
    }
}

// Conditions ------------------------------------------------------------------------------------

fn check_condition(cx: &Cx, node: &SyntaxNode) {
    if !cx.on("assignment-in-condition") {
        return;
    }
    let condition = match node.kind() {
        IF_STATEMENT | WHILE_STATEMENT | TERNARY_EXPR | ELSEIF_CLAUSE => node.children().next(),
        DO_WHILE_STATEMENT => node.children().last(),
        _ => None,
    };
    if let Some(condition) = condition {
        look_for_assignments(cx, &condition);
    }
}

fn look_for_assignments(cx: &Cx, condition: &SyntaxNode) {
    match condition.kind() {
        PREFIX_EXPR if tokens(condition).any(|token| token.kind() == BANG) => {
            if let Some(inner) = condition.children().next() {
                look_for_assignments(cx, &inner);
            }
        }
        BINARY_EXPR => {
            let logical = tokens(condition).any(|token| matches!(token.kind(), AND_AND | OR_OR | AND_KW | OR_KW));
            if logical {
                for operand in condition.children() {
                    look_for_assignments(cx, &operand);
                }
            }
        }
        ASSIGN_EXPR => {
            let operator = tokens(condition).find(|token| !token.kind().is_trivia());
            let Some(operator) = operator.filter(|token| token.kind() == ASSIGN) else {
                return;
            };
            let Some(value) = condition.children().last() else {
                return;
            };
            if is_constant_value(&value) && !was_parenthesized(condition) {
                cx.report(
                    "assignment-in-condition",
                    condition.text_range(),
                    "A value is assigned in this condition; '===' may have been meant".to_string(),
                    Fix::ReplaceAssignment {
                        operator: operator.text_range(),
                    },
                );
            }
        }
        _ => {}
    }
}

/// `if (($a = 1))` says the assignment is meant.
fn was_parenthesized(assignment: &SyntaxNode) -> bool {
    assignment.parent().is_some_and(|parent| parent.kind() == PAREN_EXPR)
}

fn is_constant_value(value: &SyntaxNode) -> bool {
    match value.kind() {
        LITERAL => true,
        NAME => matches!(text_of(value).to_ascii_lowercase().as_str(), "true" | "false" | "null"),
        _ => false,
    }
}

fn check_comparison(cx: &Cx, node: &SyntaxNode) {
    if !cx.on("incompatible-comparison") {
        return;
    }
    let Some(operator) = tokens(node).find(|token| !token.kind().is_trivia()) else {
        return;
    };
    let identical = operator.kind() == IDENTICAL;
    if !identical && operator.kind() != NOT_IDENTICAL {
        return;
    }
    let operands: Vec<SyntaxNode> = node.children().collect();
    let [left, right] = operands.as_slice() else {
        return;
    };
    let analyzer = cx.file.analyzer(node);
    let env = analyzer.env_around(node);
    let (Some(left_type), Some(right_type)) = (
        sure_type(cx, &analyzer, &env, left),
        sure_type(cx, &analyzer, &env, right),
    ) else {
        return;
    };
    let (Some(left_kinds), Some(right_kinds)) = (kinds_of(&left_type), kinds_of(&right_type)) else {
        return;
    };
    // Testing a value for `null` that cannot be is how code guards against what the types promise.
    if left_kinds.intersects(right_kinds) || left_kinds == Kinds::NULL || right_kinds == Kinds::NULL {
        return;
    }
    let outcome = if identical { "false" } else { "true" };
    cx.report(
        "incompatible-comparison",
        node.text_range(),
        format!(
            "'{}' and '{}' are never identical, so this is always {outcome}",
            left_type.display(true),
            right_type.display(true)
        ),
        Fix::None,
    );
}
