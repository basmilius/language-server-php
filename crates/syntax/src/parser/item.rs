//! Declarations: functions, classes and their members, parameters, attributes and property hooks.

use rowan::Checkpoint;

use super::expr::{self, comma_list, expr_required, name};
use super::{Parser, is_identifier_like, is_name_token, stmt, ty};
use crate::SyntaxKind::{self, *};

const MODIFIERS: &[SyntaxKind] = &[
    PUBLIC_KW,
    PROTECTED_KW,
    PRIVATE_KW,
    STATIC_KW,
    ABSTRACT_KW,
    FINAL_KW,
    READONLY_KW,
    VAR_KW,
];

/// Where a class body can safely pick up again after something it could not read.
const MEMBER_STARTS: &[SyntaxKind] = &[
    RBRACE,
    HASH_BRACKET,
    PUBLIC_KW,
    PROTECTED_KW,
    PRIVATE_KW,
    STATIC_KW,
    ABSTRACT_KW,
    FINAL_KW,
    READONLY_KW,
    VAR_KW,
    CONST_KW,
    FUNCTION_KW,
    USE_KW,
    CASE_KW,
];

pub(crate) fn attribute_lists(p: &mut Parser) {
    while p.at(HASH_BRACKET) {
        p.start(ATTRIBUTE_LIST);
        p.bump();
        comma_list(
            p,
            RBRACKET,
            |p| {
                if !is_name_token(p.current()) {
                    return false;
                }
                p.start(ATTRIBUTE);
                name(p);
                if p.at(LPAREN) {
                    expr::argument_list(p);
                }
                p.finish_node();
                true
            },
            is_name_token,
        );
        p.expect(RBRACKET, "']'");
        p.finish_node();
    }
}

/// Visibility and other modifiers, with the `(set)` of asymmetric visibility.
pub(crate) fn modifier_list(p: &mut Parser) -> bool {
    if !p.at_any(MODIFIERS) {
        return false;
    }
    p.start(MODIFIER_LIST);
    while p.at_any(MODIFIERS) {
        let visibility = matches!(p.current(), PUBLIC_KW | PROTECTED_KW | PRIVATE_KW);
        p.bump();
        if visibility && p.at(LPAREN) && p.nth_is_word(1, "set") && p.nth(2) == RPAREN && p.nth_touches_prev() {
            p.bump();
            p.bump();
            p.bump();
        }
    }
    p.finish_node();
    true
}

pub(crate) fn parameter_list(p: &mut Parser) {
    p.start(PARAMETER_LIST);
    if p.expect(LPAREN, "'('") {
        comma_list(p, RPAREN, parameter, starts_parameter);
        p.expect(RPAREN, "')'");
    }
    p.finish_node();
}

fn starts_parameter(kind: SyntaxKind) -> bool {
    kind == HASH_BRACKET
        || MODIFIERS.contains(&kind)
        || is_name_token(kind)
        || matches!(
            kind,
            ARRAY_KW | CALLABLE_KW | QUESTION | LPAREN | AMP | ELLIPSIS | VARIABLE
        )
}

fn parameter(p: &mut Parser) -> bool {
    if !starts_parameter(p.current()) {
        return false;
    }
    p.start(PARAMETER);
    attribute_lists(p);
    modifier_list(p);
    if !matches!(p.current(), VARIABLE | AMP | ELLIPSIS) {
        ty::type_(p);
    }
    p.eat(AMP);
    p.eat(ELLIPSIS);
    p.expect(VARIABLE, "Variable");
    if p.eat(ASSIGN) {
        expr_required(p);
    }
    if p.at(LBRACE) {
        property_hooks(p);
    }
    p.finish_node();
    true
}

pub(crate) fn return_type(p: &mut Parser) {
    p.start(RETURN_TYPE);
    p.bump();
    ty::type_(p);
    p.finish_node();
}

/// `function name(...) {...}`, with attributes already behind the checkpoint.
pub(crate) fn function_declaration(p: &mut Parser, checkpoint: Checkpoint) {
    p.start_at(checkpoint, FUNCTION_DECLARATION);
    p.bump();
    p.eat(AMP);
    strict_name(p);
    parameter_list(p);
    if p.at(COLON) {
        return_type(p);
    }
    if p.at(LBRACE) {
        stmt::block(p);
    } else {
        p.error_expected("'{'");
    }
    p.finish_node();
}

/// The name of a function, class or global constant, where a reserved word is an error.
fn strict_name(p: &mut Parser) {
    if p.current().is_keyword() && p.current() != READONLY_KW {
        let message = format!("Reserved word '{}' cannot be used as a name", p.current_text());
        p.error_here(message);
        name(p);
    } else {
        declaration_name(p);
    }
}

fn declaration_name(p: &mut Parser) {
    if is_identifier_like(p.current()) {
        name(p);
    } else {
        p.error_expected("Identifier");
    }
}

pub(crate) fn class_like(p: &mut Parser, checkpoint: Checkpoint) {
    let mut n = 0;
    while matches!(p.nth(n), ABSTRACT_KW | FINAL_KW | READONLY_KW) {
        n += 1;
    }
    let kind = match p.nth(n) {
        CLASS_KW => CLASS_DECLARATION,
        INTERFACE_KW => INTERFACE_DECLARATION,
        TRAIT_KW => TRAIT_DECLARATION,
        _ => ENUM_DECLARATION,
    };
    p.start_at(checkpoint, kind);
    modifier_list(p);
    p.bump();
    strict_name(p);
    if kind == ENUM_DECLARATION && p.at(COLON) {
        p.start(ENUM_BACKING_TYPE);
        p.bump();
        ty::type_(p);
        p.finish_node();
    }
    class_heads(p, kind);
    class_body(p);
    p.finish_node();
}

/// The `extends` and `implements` clauses, and which of them a kind of declaration may have.
fn class_heads(p: &mut Parser, kind: SyntaxKind) {
    if p.at(EXTENDS_KW) {
        if matches!(kind, TRAIT_DECLARATION | ENUM_DECLARATION) {
            let what = if kind == TRAIT_DECLARATION {
                "A trait"
            } else {
                "An enum"
            };
            p.error_here(format!("{what} cannot extend"));
        }
        name_clause(p, EXTENDS_CLAUSE, kind == INTERFACE_DECLARATION);
    }
    if p.at(IMPLEMENTS_KW) {
        if matches!(kind, TRAIT_DECLARATION | INTERFACE_DECLARATION) {
            let what = if kind == TRAIT_DECLARATION {
                "A trait"
            } else {
                "An interface"
            };
            p.error_here(format!("{what} cannot implement"));
        }
        name_clause(p, IMPLEMENTS_CLAUSE, true);
    }
}

fn name_clause(p: &mut Parser, kind: SyntaxKind, allow_list: bool) {
    p.start(kind);
    p.bump();
    loop {
        if is_name_token(p.current()) {
            p.start(NAMED_TYPE);
            name(p);
            p.finish_node();
        } else {
            p.error_expected("Class name");
            break;
        }
        if !p.at(COMMA) {
            break;
        }
        if !allow_list {
            p.error_here("A class can only extend one class");
        }
        p.bump();
    }
    p.finish_node();
}

pub(crate) fn anonymous_class(p: &mut Parser) {
    p.start(ANONYMOUS_CLASS);
    modifier_list(p);
    p.bump();
    if p.at(LPAREN) {
        expr::argument_list(p);
    }
    class_heads(p, CLASS_DECLARATION);
    class_body(p);
    p.finish_node();
}

fn class_body(p: &mut Parser) {
    p.start(CLASS_BODY);
    if p.expect(LBRACE, "'{'") {
        while !p.at(RBRACE) && !p.eof() {
            if matches!(p.current(), CLASS_KW | INTERFACE_KW | TRAIT_KW | NAMESPACE_KW) {
                break;
            }
            let before = p.position();
            member(p);
            if p.position() == before {
                p.error_recover(MEMBER_STARTS);
            }
        }
        p.expect(RBRACE, "'}'");
    }
    p.finish_node();
}

fn member(p: &mut Parser) {
    let checkpoint = p.declaration_checkpoint();
    let start = p.position();
    attribute_lists(p);
    let has_modifiers = modifier_list(p);
    match p.current() {
        CONST_KW => class_const(p, checkpoint),
        FUNCTION_KW => method(p, checkpoint),
        USE_KW if !has_modifiers => trait_use(p, checkpoint),
        CASE_KW => enum_case(p, checkpoint),
        kind if kind == VARIABLE || ty::at_type_start(p) && kind != RBRACE => property(p, checkpoint),
        _ => {
            if p.position() != start {
                p.start_at(checkpoint, ERROR);
                p.error_expected("Declaration");
                p.finish_node();
            }
        }
    }
}

fn class_const(p: &mut Parser, checkpoint: Checkpoint) {
    p.start_at(checkpoint, CLASS_CONST_DECLARATION);
    p.bump();
    if p.nth(1) != ASSIGN && ty::at_type_start(p) {
        ty::type_(p);
    }
    const_elements(p, false);
    stmt::semicolon(p);
    p.finish_node();
}

/// `NAME = expr, NAME = expr`, shared by class constants and `const` statements.
pub(crate) fn const_elements(p: &mut Parser, strict: bool) {
    loop {
        p.start(CONST_ELEMENT);
        if strict {
            strict_name(p);
        } else {
            declaration_name(p);
        }
        p.expect(ASSIGN, "'='");
        expr_required(p);
        p.finish_node();
        if !p.eat(COMMA) {
            break;
        }
    }
}

fn method(p: &mut Parser, checkpoint: Checkpoint) {
    p.start_at(checkpoint, METHOD_DECLARATION);
    p.bump();
    p.eat(AMP);
    declaration_name(p);
    parameter_list(p);
    if p.at(COLON) {
        return_type(p);
    }
    match p.current() {
        LBRACE => stmt::block(p),
        SEMICOLON => p.bump(),
        _ => p.error_expected("'{' or ';'"),
    }
    p.finish_node();
}

fn property(p: &mut Parser, checkpoint: Checkpoint) {
    p.start_at(checkpoint, PROPERTY_DECLARATION);
    if p.current() != VARIABLE {
        ty::type_(p);
    }
    let mut first = true;
    loop {
        p.start(PROPERTY_ELEMENT);
        p.expect(VARIABLE, "Variable");
        if p.eat(ASSIGN) {
            expr_required(p);
        }
        p.finish_node();
        if first && p.at(LBRACE) {
            property_hooks(p);
            p.finish_node();
            return;
        }
        first = false;
        if !p.eat(COMMA) {
            break;
        }
    }
    stmt::semicolon(p);
    p.finish_node();
}

fn property_hooks(p: &mut Parser) {
    p.start(PROPERTY_HOOK_LIST);
    p.bump();
    while !p.at(RBRACE) && !p.eof() {
        let before = p.position();
        property_hook(p);
        if p.position() == before {
            p.error_recover(&[RBRACE, SEMICOLON, HASH_BRACKET, FINAL_KW, AMP]);
        }
    }
    p.expect(RBRACE, "'}'");
    p.finish_node();
}

fn property_hook(p: &mut Parser) {
    if !(p.at_any(&[HASH_BRACKET, FINAL_KW, AMP]) || is_identifier_like(p.current())) {
        return;
    }
    p.start(PROPERTY_HOOK);
    attribute_lists(p);
    modifier_list(p);
    p.eat(AMP);
    declaration_name(p);
    if p.at(LPAREN) {
        parameter_list(p);
    }
    match p.current() {
        FAT_ARROW => {
            p.bump();
            expr_required(p);
            stmt::semicolon(p);
        }
        LBRACE => stmt::block(p),
        _ => {
            stmt::semicolon(p);
        }
    }
    p.finish_node();
}

fn trait_use(p: &mut Parser, checkpoint: Checkpoint) {
    p.start_at(checkpoint, TRAIT_USE);
    p.bump();
    loop {
        if is_name_token(p.current()) {
            name(p);
        } else {
            p.error_expected("Trait name");
            break;
        }
        if !p.eat(COMMA) {
            break;
        }
    }
    if p.at(LBRACE) {
        p.start(TRAIT_ADAPTATIONS);
        p.bump();
        while !p.at(RBRACE) && !p.eof() {
            let before = p.position();
            trait_adaptation(p);
            if p.position() == before {
                p.error_recover(&[RBRACE, SEMICOLON]);
                p.eat(SEMICOLON);
            }
        }
        p.expect(RBRACE, "'}'");
        p.finish_node();
    } else {
        stmt::semicolon(p);
    }
    p.finish_node();
}

fn trait_adaptation(p: &mut Parser) {
    if !is_identifier_like(p.current()) && !is_name_token(p.current()) {
        return;
    }
    let checkpoint = p.checkpoint();
    if is_name_token(p.current()) && p.nth(1) == DOUBLE_COLON {
        name(p);
        p.bump();
    }
    declaration_name(p);
    match p.current() {
        INSTEADOF_KW => {
            p.start_at(checkpoint, TRAIT_PRECEDENCE);
            p.bump();
            loop {
                if is_name_token(p.current()) {
                    name(p);
                } else {
                    p.error_expected("Trait name");
                    break;
                }
                if !p.eat(COMMA) {
                    break;
                }
            }
        }
        _ => {
            p.start_at(checkpoint, TRAIT_ALIAS);
            p.expect(AS_KW, "'as'");
            modifier_list(p);
            if is_identifier_like(p.current()) {
                name(p);
            }
        }
    }
    stmt::semicolon(p);
    p.finish_node();
}

fn enum_case(p: &mut Parser, checkpoint: Checkpoint) {
    p.start_at(checkpoint, ENUM_CASE);
    p.bump();
    declaration_name(p);
    if p.eat(ASSIGN) {
        expr_required(p);
    }
    stmt::semicolon(p);
    p.finish_node();
}
