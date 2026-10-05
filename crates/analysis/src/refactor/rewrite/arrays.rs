//! `array()` and `[]`.

use php_syntax::SyntaxKind::*;

use super::{around, caret, rewrite};
use crate::actions::edits::replace;
use crate::ast::{first_token, start, tokens};
use crate::refactor::draft::Draft;
use crate::refactor::{Rcx, Refactor};
use crate::refs::is_write_target;

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let Some(array) = around(&token).find(|node| node.kind() == ARRAY_EXPR) else {
        return;
    };
    let long = first_token(&array, ARRAY_KW).is_some();
    let open = tokens(&array).find(|token| matches!(token.kind(), LPAREN | LBRACKET));
    let close = tokens(&array)
        .filter(|token| matches!(token.kind(), RPAREN | RBRACKET))
        .last();
    let (Some(open), Some(close)) = (open, close) else {
        return;
    };
    if !long && is_write_target(&array) {
        return;
    }
    let id = format!("rewrite-array@{}", start(&array));
    let (title, keyword) = if long {
        ("Convert array() to []", first_token(&array, ARRAY_KW))
    } else {
        ("Convert [] to array()", None)
    };
    out.push(rewrite(id, title, move || {
        let mut draft = Draft::new(rcx.renv);
        if let Some(keyword) = &keyword {
            draft.here(replace(
                php_syntax::TextRange::new(keyword.text_range().start(), open.text_range().end()),
                "[",
            ));
            draft.here(replace(close.text_range(), "]"));
        } else {
            draft.here(replace(open.text_range(), "array("));
            draft.here(replace(close.text_range(), ")"));
        }
        draft.finish()
    }));
}
