//! Flipping a comparison and inverting an `if`.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode};

use super::{around, caret, has_comment, rewrite, text_of_node};
use crate::actions::edits::replace;
use crate::ast::{child_of, start, tokens};
use crate::refactor::draft::Draft;
use crate::refactor::exprs::{binary_operator, has_side_effects};
use crate::refactor::inline_variable::rank_of;
use crate::refactor::{Rcx, Refactor};

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let offset = u32::from(rcx.range.start());
    if let Some(binary) = around(&token).find(|node| node.kind() == BINARY_EXPR && flipped(node).is_some()) {
        if flip_ok(&binary) {
            let id = format!("rewrite-flip@{}", start(&binary));
            out.push(rewrite(id, "Flip the comparison", move || {
                let mut draft = Draft::new(rcx.renv);
                draft.here(replace(binary.text_range(), flip_text(rcx, &binary)?));
                draft.finish()
            }));
        }
    }
    let Some(statement) = around(&token).find(|node| {
        node.kind() == IF_STATEMENT && offset <= node.children().nth(1).map_or(u32::MAX, |body| start(&body))
    }) else {
        return;
    };
    if invert(rcx, &statement).is_ok() {
        let id = format!("rewrite-invert@{}", start(&statement));
        out.push(rewrite(id, "Invert the if", move || {
            let edits = invert(rcx, &statement)?;
            let mut draft = Draft::new(rcx.renv);
            draft.here_all(edits);
            draft.finish()
        }));
    }
}

/// What an operator becomes when its operands trade places.
fn flipped(binary: &SyntaxNode) -> Option<(SyntaxKind, &'static str)> {
    let operator = binary_operator(binary)?;
    Some(match operator {
        LT => (GT, ">"),
        GT => (LT, "<"),
        LE => (GE, ">="),
        GE => (LE, "<="),
        EQ => (EQ, "=="),
        NEQ => (
            NEQ,
            if tokens(binary).any(|token| token.text() == "<>") {
                "<>"
            } else {
                "!="
            },
        ),
        IDENTICAL => (IDENTICAL, "==="),
        NOT_IDENTICAL => (NOT_IDENTICAL, "!=="),
        _ => return None,
    })
}

fn flip_ok(binary: &SyntaxNode) -> bool {
    let operands: Vec<SyntaxNode> = binary.children().collect();
    let [left, right] = operands.as_slice() else {
        return false;
    };
    !(has_side_effects(left) && has_side_effects(right)) && !has_comment(binary)
}

fn flip_text(rcx: &Rcx<'_>, binary: &SyntaxNode) -> Result<String, String> {
    let text = rcx.cx.text;
    let operands: Vec<SyntaxNode> = binary.children().collect();
    let [left, right] = operands.as_slice() else {
        return Err("Not a comparison".to_string());
    };
    let (_, written) = flipped(binary).ok_or("Not a comparison")?;
    Ok(format!(
        "{} {written} {}",
        text_of_node(text, right),
        text_of_node(text, left)
    ))
}

/// The condition of an `if` as its opposite.
fn negated(rcx: &Rcx<'_>, condition: &SyntaxNode) -> String {
    let text = rcx.cx.text;
    let own = text_of_node(text, condition);
    match condition.kind() {
        PREFIX_EXPR if tokens(condition).any(|token| token.kind() == BANG) => condition.children().next().map_or_else(
            || format!("!({own})"),
            |inner| {
                let inner_text = text_of_node(text, &inner);
                if inner.kind() == PAREN_EXPR {
                    inner.children().next().map_or(inner_text.to_string(), |content| {
                        text_of_node(text, &content).to_string()
                    })
                } else {
                    inner_text.to_string()
                }
            },
        ),
        BINARY_EXPR => {
            let swapped = match binary_operator(condition) {
                Some(EQ) => Some("!="),
                Some(NEQ) => Some("=="),
                Some(IDENTICAL) => Some("!=="),
                Some(NOT_IDENTICAL) => Some("==="),
                _ => None,
            };
            let operands: Vec<SyntaxNode> = condition.children().collect();
            match (swapped, operands.as_slice()) {
                (Some(operator), [left, right]) => {
                    format!("{} {operator} {}", text_of_node(text, left), text_of_node(text, right))
                }
                _ => format!("!({own})"),
            }
        }
        _ if rank_of(condition).is_some() => format!("!({own})"),
        PAREN_EXPR => format!("!{own}"),
        _ => format!("!{own}"),
    }
}

fn invert(rcx: &Rcx<'_>, statement: &SyntaxNode) -> Result<Vec<crate::completion::TextEdit>, String> {
    let text = rcx.cx.text;
    if has_comment(statement) {
        return Err("There are comments in it".to_string());
    }
    if statement
        .children()
        .any(|child| child.kind() == ELSEIF_CLAUSE || child.kind() == STATEMENT_LIST)
    {
        return Err("It has an elseif or uses the alternative syntax".to_string());
    }
    let mut parts = statement.children();
    let (condition, then_body) = (parts.next().ok_or("No condition")?, parts.next().ok_or("No body")?);
    let else_clause = child_of(statement, ELSE_CLAUSE).ok_or("There is no else")?;
    let else_body = else_clause.children().next().ok_or("The else is empty")?;
    if else_body.kind() == IF_STATEMENT {
        return Err("The else holds another if".to_string());
    }
    let inverted = negated(rcx, &condition);
    Ok(vec![
        replace(condition.text_range(), inverted),
        replace(then_body.text_range(), text_of_node(text, &else_body)),
        replace(else_body.text_range(), text_of_node(text, &then_body)),
    ])
}
