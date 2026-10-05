//! The braces around the body of a control structure.

use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::{around, caret, eol, has_comment, rewrite, text_of_node, unit};
use crate::actions::edits::{indent_of, replace};
use crate::ast::{end, start};
use crate::refactor::draft::Draft;
use crate::refactor::{Rcx, Refactor};

fn is_control(kind: php_syntax::SyntaxKind) -> bool {
    matches!(
        kind,
        IF_STATEMENT | WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT | DO_WHILE_STATEMENT
    )
}

/// The bodies of a control structure with the structure each belongs to, in the order of the text.
fn bodies(control: &SyntaxNode) -> Vec<SyntaxNode> {
    if has_alternative_syntax(control) {
        return Vec::new();
    }
    let mut out: Vec<SyntaxNode> = Vec::new();
    match control.kind() {
        IF_STATEMENT => {
            if let Some(body) = control.children().nth(1) {
                out.push(body);
            }
            for clause in control
                .children()
                .filter(|child| matches!(child.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE))
            {
                let body = if clause.kind() == ELSEIF_CLAUSE {
                    clause.children().nth(1)
                } else {
                    clause.children().next()
                };
                if let Some(body) = body.filter(|body| body.kind() != IF_STATEMENT) {
                    out.push(body);
                }
            }
        }
        DO_WHILE_STATEMENT => out.extend(control.children().next()),
        _ => out.extend(control.children().last()),
    }
    out
}

fn has_alternative_syntax(control: &SyntaxNode) -> bool {
    control.children().any(|child| child.kind() == STATEMENT_LIST)
        || control.descendants().take(1).any(|_| {
            crate::ast::tokens(control)
                .any(|token| matches!(token.kind(), ENDIF_KW | ENDWHILE_KW | ENDFOR_KW | ENDFOREACH_KW))
        })
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let offset = u32::from(rcx.range.start());
    let Some(control) = around(&token).find(|node| {
        is_control(node.kind()) && {
            let first_body = bodies(node).first().map_or(u32::MAX, start);
            offset <= first_body
        }
    }) else {
        return;
    };
    let list = bodies(&control);
    if list
        .iter()
        .any(|body| body.kind() != BLOCK && body.kind() != STATEMENT_LIST)
    {
        let control = control.clone();
        out.push(rewrite(
            format!("rewrite-braces-add@{}", start(&control)),
            "Add braces",
            move || add(rcx, &control),
        ));
    }
    if list.iter().any(|body| removable(body, &control)) {
        let control = control.clone();
        out.push(rewrite(
            format!("rewrite-braces-remove@{}", start(&control)),
            "Remove braces",
            move || remove(rcx, &control),
        ));
    }
}

fn add(rcx: &Rcx<'_>, control: &SyntaxNode) -> Result<crate::refactor::Change, String> {
    let text = rcx.cx.text;
    let indent = indent_of(text, start(control) as usize);
    let (unit, eol) = (unit(rcx), eol(text));
    let mut draft = Draft::new(rcx.renv);
    for body in bodies(control) {
        if matches!(body.kind(), BLOCK | STATEMENT_LIST) {
            continue;
        }
        let statement = text_of_node(text, &body);
        draft.here(replace(
            body.text_range(),
            format!("{{{eol}{indent}{unit}{statement}{eol}{indent}}}"),
        ));
        let next = body.parent().and_then(|parent| {
            let clauses: Vec<SyntaxNode> = parent.children().collect();
            let at = clauses
                .iter()
                .position(|child| *child == body || child.children().any(|inner| inner == body))?;
            clauses
                .get(at + 1)
                .filter(|clause| matches!(clause.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE))
                .cloned()
        });
        let following = next.or_else(|| {
            let clause = body
                .parent()
                .filter(|parent| matches!(parent.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE))?;
            let sibling = clause
                .next_sibling()
                .filter(|sibling| matches!(sibling.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE))?;
            Some(sibling)
        });
        if let Some(clause) = following {
            let (from, to) = (end(&body), start(&clause));
            if text[from as usize..to as usize].trim().is_empty() && from < to {
                draft.here(replace(php_syntax::TextRange::new(from.into(), to.into()), " "));
            }
        }
    }
    draft.finish()
}

/// A block of one statement with nothing in it that braces keep apart.
fn removable(body: &SyntaxNode, control: &SyntaxNode) -> bool {
    if body.kind() != BLOCK || has_comment(body) {
        return false;
    }
    let mut statements = body.children();
    let Some(only) = statements.next() else {
        return false;
    };
    if statements.next().is_some() {
        return false;
    }
    if matches!(
        only.kind(),
        FUNCTION_DECLARATION
            | CLASS_DECLARATION
            | INTERFACE_DECLARATION
            | TRAIT_DECLARATION
            | ENUM_DECLARATION
            | EMPTY_STATEMENT
    ) {
        return false;
    }
    // Without its braces an `if` would take the `else` that belongs to the one around it.
    let last = bodies(control).last() == Some(body);
    let inner_has_else = has_token_kind(&only, ELSE_CLAUSE);
    !(only.kind() == IF_STATEMENT && !inner_has_else && !last)
}

fn has_token_kind(node: &SyntaxNode, kind: php_syntax::SyntaxKind) -> bool {
    node.children().any(|child| child.kind() == kind)
}

fn remove(rcx: &Rcx<'_>, control: &SyntaxNode) -> Result<crate::refactor::Change, String> {
    let text = rcx.cx.text;
    let mut draft = Draft::new(rcx.renv);
    for body in bodies(control) {
        if !removable(&body, control) {
            continue;
        }
        let Some(only) = body.children().next() else {
            continue;
        };
        draft.here(replace(body.text_range(), text_of_node(text, &only)));
    }
    draft.finish()
}
