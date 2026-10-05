//! `if` and `else` as a ternary, and a ternary as `if` and `else`.

use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::{around, caret, eol, has_comment, rewrite, text_of_node, unit};
use crate::actions::edits::{indent_of, replace};
use crate::ast::{child_of, first_token, start};
use crate::refactor::draft::Draft;
use crate::refactor::inline_variable::rank_of;
use crate::refactor::{Rcx, Refactor};

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let offset = u32::from(rcx.range.start());
    for node in around(&token) {
        match node.kind() {
            IF_STATEMENT => {
                let body_start = node.children().nth(1).map_or(u32::MAX, |body| start(&body));
                if offset <= body_start && to_ternary(rcx, &node).is_ok() {
                    let id = format!("rewrite-if-ternary@{}", start(&node));
                    out.push(rewrite(id, "Replace if/else with a ternary", move || {
                        let text = to_ternary(rcx, &node)?;
                        let mut draft = Draft::new(rcx.renv);
                        draft.here(replace(node.text_range(), text));
                        draft.finish()
                    }));
                    return;
                }
            }
            TERNARY_EXPR if to_if(rcx, &node).is_ok() => {
                let id = format!("rewrite-ternary-if@{}", start(&node));
                out.push(rewrite(id, "Replace the ternary with if/else", move || {
                    let (statement, text) = to_if(rcx, &node)?;
                    let mut draft = Draft::new(rcx.renv);
                    draft.here(replace(statement.text_range(), text));
                    draft.finish()
                }));
                return;
            }
            _ => {}
        }
    }
}

/// The one statement of a branch.
fn single_statement(branch: &SyntaxNode) -> Option<SyntaxNode> {
    if branch.kind() == BLOCK {
        let mut statements = branch.children();
        let only = statements.next()?;
        return statements.next().is_none().then_some(only);
    }
    Some(branch.clone())
}

fn grouped(rcx: &Rcx<'_>, expr: &SyntaxNode, needs: impl Fn(i32) -> bool) -> String {
    let text = text_of_node(rcx.cx.text, expr);
    match rank_of(expr) {
        Some(rank) if needs(rank) => format!("({text})"),
        _ => text.to_string(),
    }
}

/// `return a;` / `$x = a;` / `echo a;`: the operand it holds, and what the others of its kind must share.
fn shape(statement: &SyntaxNode, text: &str) -> Option<(&'static str, String, SyntaxNode)> {
    match statement.kind() {
        RETURN_STATEMENT => Some(("return", String::new(), statement.children().next()?)),
        ECHO_STATEMENT => {
            let mut operands = statement.children();
            let only = operands.next()?;
            operands.next().is_none().then_some(("echo", String::new(), only))
        }
        EXPR_STATEMENT => {
            let assign = statement.children().next().filter(|node| node.kind() == ASSIGN_EXPR)?;
            first_token(&assign, ASSIGN)?;
            let mut parts = assign.children();
            let (target, value) = (parts.next()?, parts.next()?);
            Some(("assign", text_of_node(text, &target).to_string(), value))
        }
        _ => None,
    }
}

fn to_ternary(rcx: &Rcx<'_>, node: &SyntaxNode) -> Result<String, String> {
    let text = rcx.cx.text;
    if has_comment(node) {
        return Err("There are comments in it".to_string());
    }
    if node.children().any(|child| child.kind() == ELSEIF_CLAUSE) {
        return Err("There is an elseif".to_string());
    }
    let mut parts = node.children();
    let (condition, then_branch) = (parts.next().ok_or("No condition")?, parts.next().ok_or("No body")?);
    let else_clause = child_of(node, ELSE_CLAUSE).ok_or("There is no else")?;
    let else_branch = else_clause.children().next().ok_or("The else is empty")?;
    let then_statement = single_statement(&then_branch).ok_or("The branch is not one statement")?;
    let else_statement = single_statement(&else_branch).ok_or("The branch is not one statement")?;
    let (kind, target, then_value) = shape(&then_statement, text).ok_or("The branches are not alike")?;
    let (other_kind, other_target, else_value) = shape(&else_statement, text).ok_or("The branches are not alike")?;
    if kind != other_kind || target != other_target {
        return Err("The branches do something different".to_string());
    }
    let low = |rank: i32| rank <= 0;
    let test = grouped(rcx, &condition, low);
    let then_text = grouped(rcx, &then_value, low);
    let else_text = grouped(rcx, &else_value, low);
    let ternary = format!("{test} ? {then_text} : {else_text}");
    Ok(match kind {
        "return" => format!("return {ternary};"),
        "echo" => format!("echo {ternary};"),
        _ => format!("{target} = {ternary};"),
    })
}

/// The statement a ternary is all of, and the `if` that replaces it.
fn to_if(rcx: &Rcx<'_>, ternary: &SyntaxNode) -> Result<(SyntaxNode, String), String> {
    let text = rcx.cx.text;
    let operands: Vec<SyntaxNode> = ternary.children().collect();
    let [condition, then_value, else_value] = operands.as_slice() else {
        return Err("A short ternary has no branch to put in an else".to_string());
    };
    if has_comment(ternary) {
        return Err("There are comments in it".to_string());
    }
    let parent = ternary.parent().ok_or("There is nothing around it")?;
    let (statement, lead) = match parent.kind() {
        RETURN_STATEMENT => (parent.clone(), "return ".to_string()),
        ECHO_STATEMENT if parent.children().count() == 1 => (parent.clone(), "echo ".to_string()),
        ASSIGN_EXPR
            if parent.children().nth(1).as_ref() == Some(ternary)
                && first_token(&parent, ASSIGN).is_some()
                && parent
                    .parent()
                    .is_some_and(|statement| statement.kind() == EXPR_STATEMENT) =>
        {
            let target = text_of_node(text, &parent.children().next().ok_or("No target")?);
            let statement = parent.parent().ok_or("No statement")?;
            (statement, format!("{target} = "))
        }
        _ => return Err("The ternary is part of something bigger".to_string()),
    };
    if !crate::refactor::exprs::is_statement_position(&statement) {
        return Err("The statement is not in a list".to_string());
    }
    let indent = indent_of(text, start(&statement) as usize);
    let (unit, eol) = (unit(rcx), eol(text));
    let condition_text = text_of_node(text, condition);
    let (then_text, else_text) = (text_of_node(text, then_value), text_of_node(text, else_value));
    Ok((
        statement,
        format!(
            "if ({condition_text}) {{{eol}{indent}{unit}{lead}{then_text};{eol}{indent}}} else {{{eol}{indent}{unit}{lead}{else_text};{eol}{indent}}}"
        ),
    ))
}
