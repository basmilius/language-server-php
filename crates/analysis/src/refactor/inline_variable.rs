//! Inline variable: a local assigned once, in a statement of its own, is replaced by its value
//! wherever it is read, and the assignment goes. A value that does something is only moved when
//! nothing can happen in between, and is never copied.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken};

use super::draft::Draft;
use super::exprs::{has_side_effects, hoist_site, is_constant_like, text_slice, token_near, variables_in};
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::actions::edits::{remove_with_lines, replace};
use crate::ast::{self, end, has_token, start};
use crate::refs::{Access, Hit, HitKind, is_write_target, variable_hits};

/// The assignment of a variable and where it is read.
struct Plan {
    statement: SyntaxNode,
    value: SyntaxNode,
    uses: Vec<SyntaxNode>,
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    if !rcx.range.is_empty() {
        return;
    }
    let Some(token) = token_near(&rcx.cx.root, u32::from(rcx.range.start())) else {
        return;
    };
    let Ok(plan) = plan_of(rcx, &token) else {
        return;
    };
    let id = format!("inline-variable@{}", start(&plan.statement));
    out.push(Refactor::new(
        id,
        "Inline variable",
        RefactorKind::Inline,
        true,
        move || inline(rcx, &token),
    ));
}

fn variable_name(token: &SyntaxToken) -> Option<String> {
    if token.kind() != VARIABLE {
        return None;
    }
    let parent = token.parent()?;
    if parent.kind() != VARIABLE_EXPR {
        return None;
    }
    let name = token.text().trim_start_matches('$').to_string();
    (name != "this").then_some(name)
}

/// Why a variable cannot be inlined, or what inlining it takes.
fn plan_of(rcx: &Rcx<'_>, token: &SyntaxToken) -> Result<Plan, String> {
    let cx = &rcx.cx;
    let name = variable_name(token).ok_or("There is no variable here")?;
    let place = token.parent().ok_or("There is no variable here")?;
    let scope =
        ast::enclosing_function(&place).ok_or("The variables of a file's top level may be read by other files")?;
    if scope.descendants().any(|node| is_dynamic(&node)) {
        return Err("The function reads its variables by name".to_string());
    }
    let taken_by_calls = super::scope::by_reference_variables(cx, &scope);
    let mut hits: Vec<Hit> = variable_hits(&cx.file, (start(&scope), end(&scope)), &name);
    for hit in &mut hits {
        if taken_by_calls.contains(&u32::from(hit.range.start())) {
            hit.access = Access::Write;
        }
    }
    let code: Vec<&Hit> = hits.iter().filter(|hit| hit.kind != HitKind::Doc).collect();
    if code.iter().any(|hit| hit.kind == HitKind::Declaration) {
        return Err("A parameter has no value to put in its place".to_string());
    }
    let writes: Vec<&&Hit> = code.iter().filter(|hit| hit.access == Access::Write).collect();
    let [write] = writes.as_slice() else {
        return Err(if writes.is_empty() {
            "The variable is never assigned".to_string()
        } else {
            "The variable is assigned more than once".to_string()
        });
    };
    let write_token = cx
        .root
        .covering_element(write.range)
        .into_token()
        .ok_or("The assignment cannot be read")?;
    let target = write_token.parent().ok_or("The assignment cannot be read")?;
    let assign = target.parent().filter(|assign| {
        assign.kind() == ASSIGN_EXPR
            && assign.children().next().as_ref() == Some(&target)
            && ast::first_token(assign, ASSIGN).is_some()
    });
    let statement = assign
        .as_ref()
        .and_then(|assign| assign.parent())
        .filter(|statement| statement.kind() == EXPR_STATEMENT && super::exprs::is_statement_position(statement))
        .ok_or("The variable is not assigned in a statement of its own")?;
    let value = assign
        .and_then(|assign| assign.children().nth(1))
        .ok_or("The assignment has no value")?;
    let mut uses = Vec::new();
    for hit in code.iter().filter(|hit| hit.access == Access::Read) {
        let element = cx.root.covering_element(hit.range);
        let Some(parent) = element.into_token().and_then(|token| token.parent()) else {
            continue;
        };
        if parent.kind() == CLOSURE_USE_VARIABLE {
            return Err("The variable is passed into a closure".to_string());
        }
        if matches!(parent.kind(), STATIC_VARIABLE | CATCH_CLAUSE | GLOBAL_STATEMENT) {
            return Err("The variable is declared elsewhere too".to_string());
        }
        if start(&parent) < end(&statement) {
            return Err("The variable is read before it is assigned".to_string());
        }
        uses.push(parent);
    }
    if uses.is_empty() {
        return Err("The variable is never read".to_string());
    }
    let container = statement.parent().ok_or("The assignment has no place")?;
    if uses
        .iter()
        .any(|node| !container.text_range().contains_range(node.text_range()))
    {
        return Err("The variable is read where the assignment may not have run".to_string());
    }
    for node in &uses {
        if is_mutated(node) {
            return Err("The variable is changed through a read".to_string());
        }
    }
    Ok(Plan { statement, value, uses })
}

fn is_dynamic(node: &SyntaxNode) -> bool {
    match node.kind() {
        VARIABLE_VARIABLE | EVAL_EXPR | INCLUDE_EXPR => true,
        CALL_EXPR => node.children().next().is_some_and(|callee| {
            callee.kind() == NAME
                && matches!(
                    ast::text_of(&callee)
                        .trim_start_matches('\\')
                        .to_ascii_lowercase()
                        .as_str(),
                    "compact" | "extract" | "get_defined_vars" | "parse_str"
                )
        }),
        _ => false,
    }
}

/// Whether the variable is the base of something that is written, so an expression cannot take its place.
fn is_mutated(variable: &SyntaxNode) -> bool {
    let mut top = variable.clone();
    while let Some(parent) = top.parent() {
        let is_base = parent.children().next().as_ref() == Some(&top);
        if matches!(parent.kind(), PROPERTY_FETCH_EXPR | INDEX_EXPR | STATIC_PROPERTY_EXPR) && is_base {
            top = parent;
        } else {
            break;
        }
    }
    if is_write_target(&top) {
        return true;
    }
    let Some(parent) = top.parent() else {
        return false;
    };
    match parent.kind() {
        ASSIGN_EXPR => parent.children().next().as_ref() == Some(&top),
        PREFIX_EXPR | POSTFIX_EXPR => has_token(&parent, INC) || has_token(&parent, DEC),
        UNSET_STATEMENT => true,
        _ => false,
    }
}

fn inline(rcx: &Rcx<'_>, token: &SyntaxToken) -> Result<Change, String> {
    let cx = &rcx.cx;
    let plan = plan_of(rcx, token)?;
    check_order(&plan)?;
    let value_text = text_slice(cx.text, &plan.value).to_string();
    let mut draft = Draft::new(rcx.renv);
    for place in &plan.uses {
        draft.here(replace(place.text_range(), replacement(&plan, &value_text, place)?));
    }
    draft.here(remove_with_lines(cx.text, plan.statement.text_range()));
    draft.finish()
}

/// Where a place sits in a string: in the simple syntax, in braces, or not in one.
enum InString {
    No,
    Simple { direct: bool },
    Braced,
}

fn in_string(place: &SyntaxNode) -> InString {
    let mut child = place.clone();
    let mut direct = true;
    while let Some(parent) = child.parent() {
        match parent.kind() {
            INTERPOLATED_STRING | HEREDOC | SHELL_EXEC_EXPR => return InString::Simple { direct },
            BRACED_INTERPOLATION | DOLLAR_BRACE_INTERPOLATION => return InString::Braced,
            kind if ast::is_function_like(kind) || matches!(kind, BLOCK | STATEMENT_LIST) => return InString::No,
            _ => {}
        }
        direct = false;
        child = parent;
    }
    InString::No
}

/// A value that can be written after the `{` of a string: a variable and what hangs off it.
fn starts_with_variable(value: &str, kind: php_syntax::SyntaxKind) -> bool {
    value.starts_with('$')
        && matches!(
            kind,
            VARIABLE_EXPR | PROPERTY_FETCH_EXPR | INDEX_EXPR | STATIC_PROPERTY_EXPR | CALL_EXPR
        )
}

/// What stands in for the variable at one place.
fn replacement(plan: &Plan, value: &str, place: &SyntaxNode) -> Result<String, String> {
    let parent = place.parent();
    match in_string(place) {
        InString::Simple { direct } => {
            if plan.value.kind() == VARIABLE_EXPR {
                return Ok(value.to_string());
            }
            if direct && starts_with_variable(value, plan.value.kind()) {
                return Ok(format!("{{{value}}}"));
            }
            return Err("The value cannot be written inside a string".to_string());
        }
        InString::Braced => {
            return if starts_with_variable(value, plan.value.kind()) && !needs_parentheses(&plan.value, place) {
                Ok(value.to_string())
            } else {
                Err("The value cannot be written inside a string".to_string())
            };
        }
        InString::No => {}
    }
    if let Some(parent) = &parent {
        if matches!(parent.kind(), ISSET_EXPR | UNSET_STATEMENT)
            && !matches!(
                plan.value.kind(),
                VARIABLE_EXPR | PROPERTY_FETCH_EXPR | INDEX_EXPR | STATIC_PROPERTY_EXPR
            )
        {
            return Err("isset and unset only take something that can be set".to_string());
        }
    }
    Ok(if needs_parentheses(&plan.value, place) {
        format!("({value})")
    } else {
        value.to_string()
    })
}

fn is_atomic(kind: php_syntax::SyntaxKind) -> bool {
    matches!(
        kind,
        VARIABLE_EXPR
            | VARIABLE_VARIABLE
            | LITERAL
            | NAME
            | CALL_EXPR
            | PROPERTY_FETCH_EXPR
            | SCOPED_ACCESS_EXPR
            | STATIC_PROPERTY_EXPR
            | INDEX_EXPR
            | ARRAY_EXPR
            | PAREN_EXPR
            | INTERPOLATED_STRING
            | HEREDOC
            | ISSET_EXPR
            | EMPTY_EXPR
            | MATCH_EXPR
            | NEW_EXPR
            | CLOSURE_EXPR
            | ARROW_FUNCTION_EXPR
    )
}

/// How tightly an expression binds: higher goes first. `None` for what stands on its own.
pub(super) fn rank_of(node: &SyntaxNode) -> Option<i32> {
    match node.kind() {
        BINARY_EXPR => Some(match super::exprs::binary_operator(node)? {
            POW => 13,
            STAR | SLASH | PERCENT => 12,
            PLUS | MINUS => 11,
            SHL | SHR => 10,
            DOT => 9,
            LT | GT | LE | GE => 8,
            EQ | NEQ | IDENTICAL | NOT_IDENTICAL | SPACESHIP => 7,
            AMP => 6,
            CARET => 5,
            PIPE => 4,
            AND_AND => 3,
            OR_OR => 2,
            COALESCE => 1,
            INSTANCEOF_KW => 14,
            AND_KW | XOR_KW | OR_KW => -2,
            _ => return None,
        }),
        TERNARY_EXPR => Some(0),
        ASSIGN_EXPR => Some(-1),
        _ => None,
    }
}

pub(super) fn needs_parentheses(value: &SyntaxNode, place: &SyntaxNode) -> bool {
    let Some(parent) = place.parent() else {
        return false;
    };
    let first = parent.children().next().as_ref() == Some(place);
    let base = first
        && matches!(
            parent.kind(),
            PROPERTY_FETCH_EXPR | INDEX_EXPR | CALL_EXPR | SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR | POSTFIX_EXPR
        );
    if base {
        return !matches!(
            value.kind(),
            VARIABLE_EXPR
                | VARIABLE_VARIABLE
                | LITERAL
                | NAME
                | CALL_EXPR
                | PROPERTY_FETCH_EXPR
                | SCOPED_ACCESS_EXPR
                | STATIC_PROPERTY_EXPR
                | INDEX_EXPR
                | ARRAY_EXPR
                | PAREN_EXPR
        );
    }
    if is_atomic(value.kind()) {
        return false;
    }
    if parent.kind() == BINARY_EXPR {
        if let (Some(inner), Some(outer)) = (rank_of(value), rank_of(&parent)) {
            let right_associative = matches!(super::exprs::binary_operator(&parent), Some(POW | COALESCE));
            return if first {
                inner < outer || inner == outer && right_associative
            } else {
                inner < outer || inner == outer && !right_associative
            };
        }
    }
    let delimited = match parent.kind() {
        ARGUMENT | ARRAY_ITEM | EXPR_STATEMENT | RETURN_STATEMENT | ECHO_STATEMENT | PAREN_EXPR | MATCH_ARM
        | THROW_EXPR | IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT | SWITCH_STATEMENT | MATCH_EXPR
        | BRACED_INTERPOLATION | CASE_CLAUSE | PRINT_EXPR | YIELD_EXPR | FOREACH_STATEMENT => true,
        ASSIGN_EXPR => !first,
        INDEX_EXPR => !first,
        _ => false,
    };
    !delimited
}

/// What is in the way of the value being read later than it is assigned: something that changes
/// what it gives, or that it changes itself.
struct Value {
    effects: bool,
    constant: bool,
    local: bool,
    reads: Vec<String>,
}

impl Value {
    fn of(value: &SyntaxNode) -> Value {
        let state = value.descendants().any(|node| {
            matches!(
                node.kind(),
                PROPERTY_FETCH_EXPR | STATIC_PROPERTY_EXPR | CALL_EXPR | NEW_EXPR
            )
        }) || value
            .descendants()
            .any(|node| node.kind() == VARIABLE_EXPR && ast::text_of(&node) == "$this");
        Value {
            effects: has_side_effects(value),
            constant: is_constant_like(value),
            local: !state,
            reads: variables_in(value),
        }
    }

    fn is_changed_by(&self, node: &SyntaxNode) -> bool {
        match node.kind() {
            VARIABLE_EXPR => is_write_target(node) && variables_in(node).iter().any(|name| self.reads.contains(name)),
            CALL_EXPR | NEW_EXPR | INCLUDE_EXPR | EVAL_EXPR | YIELD_EXPR | YIELD_FROM_EXPR | PRINT_EXPR | EXIT_EXPR
            | CLONE_EXPR => {
                self.effects
                    || !self.constant
                        && (!self.local || variables_in(node).iter().any(|name| self.reads.contains(name)))
            }
            ASSIGN_EXPR => !targets_a_local(node) && (self.effects || !self.constant && !self.local),
            PREFIX_EXPR | POSTFIX_EXPR if has_token(node, INC) || has_token(node, DEC) => {
                !targets_a_local(node) && (self.effects || !self.constant && !self.local)
            }
            _ => false,
        }
    }
}

/// An assignment or increment of a plain variable, which only matters to a value that reads that variable.
fn targets_a_local(node: &SyntaxNode) -> bool {
    node.children()
        .next()
        .is_some_and(|target| target.kind() == VARIABLE_EXPR)
}

/// Whether the value may be moved to where the variable is read.
fn check_order(plan: &Plan) -> Result<(), String> {
    let value = Value::of(&plan.value);
    if value.effects && plan.uses.len() > 1 {
        return Err("The value does something, so it cannot be copied to every place".to_string());
    }
    let Some(container) = plan.statement.parent() else {
        return Err("The assignment has no place".to_string());
    };
    for place in &plan.uses {
        let mut from = end(&plan.statement);
        let mut to = start(place);
        if let Some(outer) = outer_loop(place, &plan.statement) {
            from = from.min(start(&outer));
            to = to.max(end(&outer));
        }
        let in_the_way = container.descendants().any(|node| {
            let at = start(&node);
            at >= from && at < to && !place.ancestors().any(|ancestor| ancestor == node) && value.is_changed_by(&node)
        });
        if in_the_way {
            return Err("What the value reads or does may change before the variable is read".to_string());
        }
        if value.effects {
            let site =
                hoist_site(place).map_err(|_| "The value does something, so it has to stay where it is".to_string())?;
            if site.container != container {
                return Err("The value only runs sometimes where the variable is read".to_string());
            }
            let leaves = container.descendants().any(|node| {
                start(&node) >= end(&plan.statement)
                    && end(&node) <= start(place)
                    && matches!(
                        node.kind(),
                        RETURN_STATEMENT
                            | BREAK_STATEMENT
                            | CONTINUE_STATEMENT
                            | GOTO_STATEMENT
                            | THROW_EXPR
                            | EXIT_EXPR
                    )
            });
            if leaves {
                return Err("Code between the assignment and the read may leave first".to_string());
            }
            if place
                .ancestors()
                .any(|ancestor| matches!(ancestor.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
            {
                return Err("The value would run when the closure runs".to_string());
            }
        }
    }
    Ok(())
}

/// The outermost loop that holds a place and not the assignment.
fn outer_loop(place: &SyntaxNode, assignment: &SyntaxNode) -> Option<SyntaxNode> {
    place
        .ancestors()
        .filter(|node| {
            matches!(
                node.kind(),
                WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT
            ) && !node.text_range().contains_range(assignment.text_range())
        })
        .last()
}
