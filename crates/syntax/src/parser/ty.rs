//! Type declarations: nullable, union, intersection and the parenthesized groups of DNF types.

use super::{Parser, is_name_token};
use crate::SyntaxKind::*;

pub(crate) fn at_type_start(p: &Parser) -> bool {
    let kind = p.current();
    is_name_token(kind) || matches!(kind, ARRAY_KW | CALLABLE_KW | STATIC_KW | QUESTION | LPAREN)
}

/// Whether an `&` after a type is the by-reference marker of a parameter and not an intersection.
fn ampersand_is_reference(p: &Parser) -> bool {
    matches!(p.nth(1), VARIABLE | ELLIPSIS | AMP)
}

pub(crate) fn type_(p: &mut Parser) {
    if p.at(QUESTION) {
        p.start(NULLABLE_TYPE);
        p.bump();
        type_atom(p);
        p.finish_node();
        return;
    }
    let checkpoint = p.checkpoint();
    type_atom(p);
    if p.at(PIPE) {
        p.start_at(checkpoint, UNION_TYPE);
        while p.eat(PIPE) {
            type_atom(p);
        }
        p.finish_node();
    } else if p.at(AMP) && !ampersand_is_reference(p) {
        p.start_at(checkpoint, INTERSECTION_TYPE);
        while p.at(AMP) && !ampersand_is_reference(p) {
            p.bump();
            type_atom(p);
        }
        p.finish_node();
    }
}

fn type_atom(p: &mut Parser) {
    if p.at(LPAREN) {
        p.start(PAREN_TYPE);
        p.bump();
        type_atom(p);
        while p.at(AMP) {
            p.bump();
            type_atom(p);
        }
        p.expect(RPAREN, "')'");
        p.finish_node();
        return;
    }
    p.start(NAMED_TYPE);
    if is_name_token(p.current()) || matches!(p.current(), ARRAY_KW | CALLABLE_KW | STATIC_KW) {
        super::expr::name(p);
    } else {
        p.error_expected("Type");
    }
    p.finish_node();
}
