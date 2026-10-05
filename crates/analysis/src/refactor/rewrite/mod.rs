//! Rewrites: the same program written another way, offered where the cursor is on the code that
//! can be rewritten. Each is cheap, so its edits are worked out as soon as it is offered.

mod arguments;
mod arrays;
mod braces;
mod comparisons;
mod conditionals;
mod declarations;
mod strings;
mod switch_match;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode, SyntaxToken};

use super::exprs::token_near;
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::ast::{end, start};

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    arrays::offer(rcx, out);
    conditionals::offer(rcx, out);
    switch_match::offer(rcx, out);
    strings::offer(rcx, out);
    braces::offer(rcx, out);
    declarations::offer(rcx, out);
    comparisons::offer(rcx, out);
    arguments::offer(rcx, out);
}

/// A rewrite that is offered as it is, since its edits cost nothing.
pub(super) fn rewrite<'a>(
    id: String,
    title: impl Into<String>,
    run: impl Fn() -> Result<Change, String> + 'a,
) -> Refactor<'a> {
    Refactor::new(id, title, RefactorKind::Rewrite, false, run)
}

/// The token a cursor is on.
pub(super) fn caret(rcx: &Rcx<'_>) -> Option<SyntaxToken> {
    token_near(&rcx.cx.root, u32::from(rcx.range.start()))
}

/// The nodes around a token, innermost first.
pub(super) fn around(token: &SyntaxToken) -> impl Iterator<Item = SyntaxNode> {
    token.parent().into_iter().flat_map(|parent| parent.ancestors())
}

pub(super) fn has_comment(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens().any(|element| match element {
        SyntaxElement::Token(token) => matches!(token.kind(), COMMENT | BLOCK_COMMENT | DOC_COMMENT),
        SyntaxElement::Node(_) => false,
    })
}

pub(super) fn text_of_node<'a>(text: &'a str, node: &SyntaxNode) -> &'a str {
    &text[start(node) as usize..end(node) as usize]
}

/// The newline a file uses.
pub(super) fn eol(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// What one level of indentation is in the file: what its first indented line has, else the options.
pub(super) fn unit(rcx: &Rcx<'_>) -> String {
    let found = rcx
        .cx
        .text
        .lines()
        .find(|line| line.starts_with(' ') || line.starts_with('\t') && !line.trim().is_empty());
    match found {
        Some(line) if line.starts_with('\t') => "\t".to_string(),
        Some(line) => {
            let width = line.len() - line.trim_start_matches(' ').len();
            " ".repeat(if width % 4 == 0 { 4 } else { width.clamp(1, 8) })
        }
        None => rcx.renv.format.indent.unit(),
    }
}
