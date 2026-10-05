//! A declaration of several things as several declarations, and the other way round.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::{around, caret, eol, has_comment, rewrite, text_of_node};
use crate::actions::edits::{indent_of, replace};
use crate::ast::{child_of, end, start, tokens};
use crate::refactor::draft::Draft;
use crate::refactor::{Rcx, Refactor};

fn element_kind(kind: php_syntax::SyntaxKind) -> Option<php_syntax::SyntaxKind> {
    match kind {
        PROPERTY_DECLARATION => Some(PROPERTY_ELEMENT),
        CLASS_CONST_DECLARATION | CONST_STATEMENT => Some(CONST_ELEMENT),
        STATIC_VARIABLE_STATEMENT => Some(STATIC_VARIABLE),
        GLOBAL_STATEMENT => Some(VARIABLE_EXPR),
        USE_STATEMENT => Some(USE_CLAUSE),
        _ => None,
    }
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let Some(declaration) = around(&token).find(|node| element_kind(node.kind()).is_some()) else {
        return;
    };
    if split(rcx, &declaration).is_ok() {
        let id = format!("rewrite-split@{}", start(&declaration));
        let node = declaration.clone();
        out.push(rewrite(id, "Split into separate declarations", move || {
            let text = split(rcx, &node)?;
            let mut draft = Draft::new(rcx.renv);
            draft.here(replace(node.text_range(), text));
            draft.finish()
        }));
    }
    if let Ok((range, text)) = join(rcx, &declaration) {
        let id = format!("rewrite-join@{}", start(&declaration));
        out.push(rewrite(id, "Join adjacent declarations", move || {
            let mut draft = Draft::new(rcx.renv);
            draft.here(replace(range, text.clone()));
            draft.finish()
        }));
    }
}

/// Everything of a declaration that comes before its first element: modifiers, type, keyword.
fn head(text: &str, declaration: &SyntaxNode, first: &SyntaxNode, with_doc: bool) -> String {
    let mut from = start(declaration);
    if !with_doc {
        let after_doc = tokens(declaration)
            .take_while(|token| token.text_range().start() < first.text_range().start())
            .filter(|token| matches!(token.kind(), DOC_COMMENT | COMMENT | BLOCK_COMMENT))
            .last();
        if let Some(doc) = after_doc {
            from = u32::from(doc.text_range().end());
        }
    }
    text[from as usize..start(first) as usize].trim().to_string()
}

fn elements(declaration: &SyntaxNode) -> Vec<SyntaxNode> {
    let Some(kind) = element_kind(declaration.kind()) else {
        return Vec::new();
    };
    let mut all: Vec<SyntaxNode> = declaration.children().filter(|child| child.kind() == kind).collect();
    if declaration.kind() == USE_STATEMENT && child_of(declaration, USE_GROUP).is_some() {
        all.clear();
    }
    all
}

fn split(rcx: &Rcx<'_>, declaration: &SyntaxNode) -> Result<String, String> {
    let text = rcx.cx.text;
    let all = elements(declaration);
    if all.len() < 2 {
        return Err("There is one thing declared".to_string());
    }
    let inner_comment = all.iter().any(has_comment);
    if inner_comment {
        return Err("There are comments between the elements".to_string());
    }
    let indent = indent_of(text, start(declaration) as usize);
    let eol = eol(text);
    let mut lines: Vec<String> = Vec::new();
    for (position, element) in all.iter().enumerate() {
        let head = head(text, declaration, &all[0], position == 0);
        let body = text_of_node(text, element);
        let line = if position == 0 {
            format!("{head} {body};")
        } else {
            format!("{indent}{head} {body};")
        };
        lines.push(line.replace(" ;", ";"));
    }
    Ok(lines.join(eol))
}

/// The adjacent declarations of the same kind, with the same head, around one.
fn join(rcx: &Rcx<'_>, declaration: &SyntaxNode) -> Result<(TextRange, String), String> {
    let text = rcx.cx.text;
    if !matches!(declaration.kind(), PROPERTY_DECLARATION | CLASS_CONST_DECLARATION) {
        return Err("Only members are joined".to_string());
    }
    let first_of = |node: &SyntaxNode| elements(node).into_iter().next();
    let Some(own) = first_of(declaration) else {
        return Err("Nothing to join".to_string());
    };
    let own_head = head(text, declaration, &own, false);
    let plain = |node: &SyntaxNode| {
        node.kind() == declaration.kind()
            && !has_comment(node)
            && child_of(node, ATTRIBUTE_LIST).is_none()
            && first_of(node).is_some_and(|element| head(text, node, &element, false) == own_head)
    };
    let siblings: Vec<SyntaxNode> = declaration
        .parent()
        .map(|parent| parent.children().collect())
        .unwrap_or_default();
    let at = siblings
        .iter()
        .position(|node| node == declaration)
        .ok_or("Not in a list")?;
    let mut low = at;
    while low > 0 && plain(&siblings[low - 1]) && gap_is_blank(text, &siblings[low - 1], &siblings[low]) {
        low -= 1;
    }
    let mut high = at;
    while high + 1 < siblings.len()
        && plain(&siblings[high + 1])
        && gap_is_blank(text, &siblings[high], &siblings[high + 1])
    {
        high += 1;
    }
    if low == high || !plain(&siblings[low]) {
        return Err("There is nothing next to it to join".to_string());
    }
    let run = &siblings[low..=high];
    let mut parts: Vec<String> = Vec::new();
    for node in run {
        for element in elements(node) {
            parts.push(text_of_node(text, &element).to_string());
        }
    }
    let first = &run[0];
    let first_head = head(text, first, &first_of(first).ok_or("Nothing to join")?, true);
    let range = TextRange::new(start(first).into(), end(&run[run.len() - 1]).into());
    Ok((range, format!("{first_head} {};", parts.join(", "))))
}

/// Only whitespace with at most one line break between two declarations.
fn gap_is_blank(text: &str, left: &SyntaxNode, right: &SyntaxNode) -> bool {
    let gap = &text[end(left) as usize..start(right) as usize];
    gap.trim().is_empty() && gap.matches('\n').count() <= 1
}
