//! Expressions: a Pratt parser over PHP's operator table, with the postfix chain (calls, indexes,
//! member access) folded into the primary.

use rowan::Checkpoint;

use super::{Parser, is_identifier_like, is_name_token, item, stmt, string};
use crate::SyntaxKind::{self, *};

/// Tokens that end a list early: a bracket that was never closed must not swallow the next statement.
const LIST_STOPS: &[SyntaxKind] = &[SEMICOLON, RBRACE, LBRACE, CLOSE_TAG, EOF];

pub(crate) fn can_start_expr(kind: SyntaxKind) -> bool {
    is_name_token(kind)
        || matches!(
            kind,
            VARIABLE
                | DOLLAR
                | INT_LITERAL
                | FLOAT_LITERAL
                | STRING_LITERAL
                | MAGIC_CONSTANT
                | DOUBLE_QUOTE
                | BACKTICK
                | HEREDOC_START
                | LPAREN
                | LBRACKET
                | CAST
                | PLUS
                | MINUS
                | BANG
                | TILDE
                | AT
                | INC
                | DEC
                | HASH_BRACKET
                | NEW_KW
                | CLONE_KW
                | PRINT_KW
                | YIELD_KW
                | THROW_KW
                | INCLUDE_KW
                | INCLUDE_ONCE_KW
                | REQUIRE_KW
                | REQUIRE_ONCE_KW
                | ISSET_KW
                | EMPTY_KW
                | EVAL_KW
                | EXIT_KW
                | ARRAY_KW
                | LIST_KW
                | FUNCTION_KW
                | FN_KW
                | STATIC_KW
                | MATCH_KW
                | READONLY_KW
        )
}

pub(crate) fn expr(p: &mut Parser) -> Option<SyntaxKind> {
    expr_bp(p, 0)
}

/// An expression that has to be there: reports it missing otherwise.
pub(crate) fn expr_required(p: &mut Parser) -> bool {
    if expr(p).is_some() {
        return true;
    }
    p.error_expected("Expression");
    false
}

pub(crate) fn name(p: &mut Parser) {
    p.start(NAME);
    p.bump();
    p.finish_node();
}

pub(crate) fn is_assignable(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        VARIABLE_EXPR
            | VARIABLE_VARIABLE
            | INDEX_EXPR
            | PROPERTY_FETCH_EXPR
            | STATIC_PROPERTY_EXPR
            | ARRAY_EXPR
            | LIST_EXPR
    )
}

fn is_assign_op(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        ASSIGN
            | PLUS_ASSIGN
            | MINUS_ASSIGN
            | STAR_ASSIGN
            | SLASH_ASSIGN
            | DOT_ASSIGN
            | PERCENT_ASSIGN
            | POW_ASSIGN
            | COALESCE_ASSIGN
            | AMP_ASSIGN
            | PIPE_ASSIGN
            | CARET_ASSIGN
            | SHL_ASSIGN
            | SHR_ASSIGN
    )
}

/// Binding power of the operand of `!`: below `instanceof`, above arithmetic.
const BANG_BP: u8 = 19;
/// Binding power of the operand of `-`, `+`, `~`, `@`, a cast or `++`: above `instanceof`, below `**`.
const UNARY_BP: u8 = 21;
const ASSIGN_BP: u8 = 4;
const TERNARY_BP: u8 = 5;
const INSTANCEOF_BP: u8 = 20;
/// The operand of `clone` is a primary with its postfix chain and nothing looser.
const CLONE_BP: u8 = 24;

/// Left and right binding power of a binary operator.
fn infix(kind: SyntaxKind) -> Option<(u8, u8)> {
    Some(match kind {
        OR_KW => (1, 2),
        XOR_KW => (2, 3),
        AND_KW => (3, 4),
        COALESCE => (6, 6),
        OR_OR => (7, 8),
        AND_AND => (8, 9),
        PIPE => (9, 10),
        CARET => (10, 11),
        AMP => (11, 12),
        EQ | NEQ | IDENTICAL | NOT_IDENTICAL | SPACESHIP => (12, 13),
        LT | LE | GT | GE => (13, 14),
        PIPE_GT => (14, 15),
        DOT => (15, 16),
        SHL | SHR => (16, 17),
        PLUS | MINUS => (17, 18),
        STAR | SLASH | PERCENT => (18, 19),
        POW => (23, 23),
        _ => return None,
    })
}

pub(crate) fn expr_bp(p: &mut Parser, min_bp: u8) -> Option<SyntaxKind> {
    if !p.enter() {
        return None;
    }
    let result = expr_bp_inner(p, min_bp);
    p.leave();
    result
}

fn expr_bp_inner(p: &mut Parser, min_bp: u8) -> Option<SyntaxKind> {
    let checkpoint = p.checkpoint();
    let mut lhs = unary_or_primary(p, checkpoint)?;
    loop {
        let op = p.current();
        if is_assign_op(op) && is_assignable(lhs) {
            p.start_at(checkpoint, ASSIGN_EXPR);
            p.bump();
            if op == ASSIGN && p.eat(AMP) {
                match expr_bp(p, CLONE_BP) {
                    Some(kind) if is_assignable(kind) || kind == CALL_EXPR => {}
                    Some(_) => p.error_here("Only variables and calls can be assigned by reference"),
                    None => p.error_expected("Expression"),
                }
            } else {
                expr_required_bp(p, ASSIGN_BP);
            }
            p.finish_node();
            lhs = ASSIGN_EXPR;
            continue;
        }
        if matches!(op, INC | DEC) && is_assignable(lhs) {
            p.start_at(checkpoint, POSTFIX_EXPR);
            p.bump();
            p.finish_node();
            lhs = POSTFIX_EXPR;
            continue;
        }
        if op == QUESTION {
            if TERNARY_BP < min_bp {
                break;
            }
            p.start_at(checkpoint, TERNARY_EXPR);
            p.bump();
            if !p.at(COLON) {
                expr_required_bp(p, 0);
            }
            p.expect(COLON, "':'");
            expr_required_bp(p, TERNARY_BP + 1);
            p.finish_node();
            lhs = TERNARY_EXPR;
            continue;
        }
        if op == INSTANCEOF_KW {
            if INSTANCEOF_BP < min_bp {
                break;
            }
            p.start_at(checkpoint, BINARY_EXPR);
            p.bump();
            class_reference_for_instanceof(p);
            p.finish_node();
            lhs = BINARY_EXPR;
            continue;
        }
        let Some((left_bp, right_bp)) = infix(op) else {
            break;
        };
        if left_bp < min_bp {
            break;
        }
        p.start_at(checkpoint, BINARY_EXPR);
        p.bump();
        expr_required_bp(p, right_bp);
        p.finish_node();
        lhs = BINARY_EXPR;
    }
    Some(lhs)
}

fn expr_required_bp(p: &mut Parser, min_bp: u8) {
    if expr_bp(p, min_bp).is_none() {
        p.error_expected("Expression");
    }
}

/// The right side of `instanceof` and the class of `new`: a name, which may carry a static property
/// (`static::$class`), a variable with property and index accesses, or a parenthesized expression.
fn class_name_reference(p: &mut Parser) {
    match p.current() {
        kind if is_name_token(kind) || kind == STATIC_KW => {
            let checkpoint = p.checkpoint();
            name(p);
            if p.at(DOUBLE_COLON) && matches!(p.nth(1), VARIABLE | DOLLAR) {
                p.start_at(checkpoint, STATIC_PROPERTY_EXPR);
                p.bump();
                variable(p);
                p.finish_node();
                new_variable_tail(p, checkpoint);
            }
        }
        VARIABLE | DOLLAR => new_variable(p),
        LPAREN => {
            p.start(PAREN_EXPR);
            p.bump();
            expr_required(p);
            p.expect(RPAREN, "')'");
            p.finish_node();
        }
        _ => p.error_expected("Class name"),
    }
}

/// `instanceof` also takes any expression with its postfix chain, as `$a instanceof $b->c`.
fn class_reference_for_instanceof(p: &mut Parser) {
    if matches!(p.current(), VARIABLE | DOLLAR) || is_name_token(p.current()) || p.at_any(&[STATIC_KW, LPAREN]) {
        class_name_reference(p);
    } else if expr_bp(p, CLONE_BP).is_none() {
        p.error_expected("Class name");
    }
}

fn unary_or_primary(p: &mut Parser, checkpoint: Checkpoint) -> Option<SyntaxKind> {
    match p.current() {
        BANG => Some(prefix(p, checkpoint, BANG_BP)),
        PLUS | MINUS | TILDE | AT | INC | DEC => Some(prefix(p, checkpoint, UNARY_BP)),
        CAST => {
            let is_void = p
                .current_text()
                .trim_matches(['(', ')', ' ', '\t'])
                .eq_ignore_ascii_case("void");
            if is_void && !std::mem::take(&mut p.void_cast_allowed) {
                p.error_here("'(void)' cast is only allowed as a statement");
            }
            p.start_at(checkpoint, CAST_EXPR);
            p.bump();
            expr_required_bp(p, UNARY_BP);
            p.finish_node();
            Some(CAST_EXPR)
        }
        PRINT_KW => Some(keyword_operand(p, checkpoint, PRINT_EXPR, ASSIGN_BP)),
        THROW_KW => Some(keyword_operand(p, checkpoint, THROW_EXPR, 0)),
        INCLUDE_KW | INCLUDE_ONCE_KW | REQUIRE_KW | REQUIRE_ONCE_KW => {
            Some(keyword_operand(p, checkpoint, INCLUDE_EXPR, 0))
        }
        YIELD_KW => Some(yield_expr(p, checkpoint)),
        CLONE_KW => Some(clone_expr(p, checkpoint)),
        _ => primary(p, checkpoint),
    }
}

fn prefix(p: &mut Parser, checkpoint: Checkpoint, bp: u8) -> SyntaxKind {
    p.start_at(checkpoint, PREFIX_EXPR);
    p.bump();
    expr_required_bp(p, bp);
    p.finish_node();
    PREFIX_EXPR
}

fn keyword_operand(p: &mut Parser, checkpoint: Checkpoint, kind: SyntaxKind, bp: u8) -> SyntaxKind {
    p.start_at(checkpoint, kind);
    p.bump();
    expr_required_bp(p, bp);
    p.finish_node();
    kind
}

fn yield_expr(p: &mut Parser, checkpoint: Checkpoint) -> SyntaxKind {
    if p.nth_is_word(1, "from") {
        p.start_at(checkpoint, YIELD_FROM_EXPR);
        p.bump();
        p.bump();
        expr_required_bp(p, ASSIGN_BP);
        p.finish_node();
        return YIELD_FROM_EXPR;
    }
    p.start_at(checkpoint, YIELD_EXPR);
    p.bump();
    if can_start_expr(p.current()) {
        expr_bp(p, ASSIGN_BP);
        if p.eat(FAT_ARROW) {
            expr_required_bp(p, ASSIGN_BP);
        }
    }
    p.finish_node();
    YIELD_EXPR
}

fn clone_expr(p: &mut Parser, checkpoint: Checkpoint) -> SyntaxKind {
    p.start_at(checkpoint, CLONE_EXPR);
    p.bump();
    if p.at(LPAREN) && clone_is_call(p) {
        argument_list(p);
    } else {
        expr_required_bp(p, CLONE_BP);
    }
    p.finish_node();
    CLONE_EXPR
}

/// Whether `clone(` is a call with arguments, as `clone($object, [...])`, `clone(object: $x)` and
/// `clone(...)` are, and not a parenthesized operand.
fn clone_is_call(p: &Parser) -> bool {
    p.nth(1) == ELLIPSIS
        || p.nth(1) == QUESTION && matches!(p.nth(2), COMMA | RPAREN)
        || (is_identifier_like(p.nth(1)) && p.nth(2) == COLON)
        || paren_has_top_level_comma(p)
}

/// Whether the parenthesis at the cursor holds a comma at its own level.
fn paren_has_top_level_comma(p: &Parser) -> bool {
    let mut depth = 0u32;
    let mut n = 0;
    loop {
        match p.nth(n) {
            EOF => return false,
            LPAREN | LBRACKET | LBRACE | CURLY_OPEN | DOLLAR_OPEN_CURLY => depth += 1,
            RPAREN | RBRACKET | RBRACE => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return false;
                }
            }
            COMMA if depth == 1 => return true,
            SEMICOLON if depth <= 1 => return false,
            _ => {}
        }
        n += 1;
        if n > 4096 {
            return false;
        }
    }
}

fn primary(p: &mut Parser, checkpoint: Checkpoint) -> Option<SyntaxKind> {
    let kind = match p.current() {
        VARIABLE | DOLLAR => variable(p),
        INT_LITERAL | FLOAT_LITERAL | STRING_LITERAL | MAGIC_CONSTANT => {
            if p.at(STRING_LITERAL) && p.current_text().trim_start_matches(['b', 'B']).starts_with('"') {
                let (offset, text) = (p.current_offset(), p.current_text());
                string::check_escapes(p, offset, text);
            }
            p.start(LITERAL);
            p.bump();
            p.finish_node();
            LITERAL
        }
        DOUBLE_QUOTE => {
            string::interpolated_string(p);
            INTERPOLATED_STRING
        }
        BACKTICK => {
            string::shell_exec(p);
            SHELL_EXEC_EXPR
        }
        HEREDOC_START => {
            string::heredoc(p);
            HEREDOC
        }
        LPAREN => {
            p.start(PAREN_EXPR);
            p.bump();
            expr_required(p);
            p.expect(RPAREN, "')'");
            p.finish_node();
            PAREN_EXPR
        }
        LBRACKET => array_expr(p),
        ARRAY_KW if p.nth(1) == LPAREN => array_expr(p),
        LIST_KW => list_expr(p),
        kind if is_name_token(kind) => {
            name(p);
            NAME
        }
        READONLY_KW if p.nth(1) == LPAREN => {
            name(p);
            NAME
        }
        STATIC_KW => {
            if matches!(p.nth(1), FUNCTION_KW | FN_KW) {
                return Some(closure(p, checkpoint));
            }
            name(p);
            NAME
        }
        FUNCTION_KW | FN_KW => return Some(closure(p, checkpoint)),
        HASH_BRACKET => {
            item::attribute_lists(p);
            if matches!(p.current(), FUNCTION_KW | FN_KW | STATIC_KW) {
                return Some(closure(p, checkpoint));
            }
            p.error_expected("Function");
            return Some(ERROR);
        }
        MATCH_KW => match_expr(p),
        NEW_KW => {
            if !new_expr(p) {
                return Some(NEW_EXPR);
            }
            NEW_EXPR
        }
        ISSET_KW => {
            parenthesized_list(p, ISSET_EXPR);
            ISSET_EXPR
        }
        EMPTY_KW => {
            parenthesized_list(p, EMPTY_EXPR);
            EMPTY_EXPR
        }
        EVAL_KW => {
            parenthesized_list(p, EVAL_EXPR);
            EVAL_EXPR
        }
        EXIT_KW => {
            p.start(EXIT_EXPR);
            p.bump();
            if p.at(LPAREN) && p.nth(1) == ELLIPSIS {
                argument_list(p);
            } else if p.at(LPAREN) {
                p.bump();
                if !p.at(RPAREN) {
                    expr_required(p);
                }
                p.expect(RPAREN, "')'");
            }
            p.finish_node();
            EXIT_EXPR
        }
        _ => return None,
    };
    Some(postfix(p, checkpoint, kind))
}

/// `isset(...)`, `empty(...)` and `eval(...)`.
fn parenthesized_list(p: &mut Parser, kind: SyntaxKind) {
    p.start(kind);
    p.bump();
    if p.expect(LPAREN, "'('") {
        comma_list(p, RPAREN, |p| expr(p).is_some(), can_start_expr);
        p.expect(RPAREN, "')'");
    }
    p.finish_node();
}

/// Calls, indexes and member access after an operand.
fn postfix(p: &mut Parser, checkpoint: Checkpoint, mut kind: SyntaxKind) -> SyntaxKind {
    loop {
        match p.current() {
            LBRACKET => {
                p.start_at(checkpoint, INDEX_EXPR);
                p.bump();
                if !p.at(RBRACKET) {
                    expr_required(p);
                }
                p.expect(RBRACKET, "']'");
                p.finish_node();
                kind = INDEX_EXPR;
            }
            ARROW | NULLSAFE_ARROW => {
                p.start_at(checkpoint, PROPERTY_FETCH_EXPR);
                p.bump();
                member_name(p, "Property name");
                p.finish_node();
                kind = PROPERTY_FETCH_EXPR;
            }
            DOUBLE_COLON => {
                let is_static_property = matches!(p.nth(1), VARIABLE | DOLLAR);
                kind = if is_static_property {
                    STATIC_PROPERTY_EXPR
                } else {
                    SCOPED_ACCESS_EXPR
                };
                p.start_at(checkpoint, kind);
                p.bump();
                if p.at(CLASS_KW) {
                    name(p);
                } else {
                    member_name(p, "Member name");
                }
                p.finish_node();
            }
            LPAREN => {
                p.start_at(checkpoint, CALL_EXPR);
                argument_list(p);
                p.finish_node();
                kind = CALL_EXPR;
            }
            _ => return kind,
        }
    }
}

/// What follows `->` or `::`: a name (reserved words included), a variable or `{expr}`.
fn member_name(p: &mut Parser, what: &str) {
    match p.current() {
        kind if is_identifier_like(kind) => name(p),
        VARIABLE | DOLLAR => {
            variable(p);
        }
        LBRACE => {
            p.bump();
            expr_required(p);
            p.expect(RBRACE, "'}'");
        }
        _ => p.error_expected(what),
    }
}

/// `$a`, `$$a` and `${expr}`.
pub(crate) fn variable(p: &mut Parser) -> SyntaxKind {
    if p.at(VARIABLE) {
        p.start(VARIABLE_EXPR);
        p.bump();
        p.finish_node();
        return VARIABLE_EXPR;
    }
    p.start(VARIABLE_VARIABLE);
    p.bump();
    match p.current() {
        VARIABLE | DOLLAR => {
            variable(p);
        }
        LBRACE => {
            p.bump();
            expr_required(p);
            p.expect(RBRACE, "'}'");
        }
        _ => p.error_expected("Variable"),
    }
    p.finish_node();
    VARIABLE_VARIABLE
}

/// A comma separated list up to `close`. `item` parses one entry and says whether there was one;
/// `starts_item` tells a missing comma from the end of the list.
pub(crate) fn comma_list(
    p: &mut Parser,
    close: SyntaxKind,
    mut item: impl FnMut(&mut Parser) -> bool,
    starts_item: fn(SyntaxKind) -> bool,
) {
    while !p.at(close) && !p.at_any(LIST_STOPS) {
        if !item(p) {
            if p.at(COMMA) {
                p.error_here("Unexpected ','");
            } else {
                p.error_bump();
            }
        }
        if p.eat(COMMA) {
            continue;
        }
        if p.at(close) || !starts_item(p.current()) {
            break;
        }
        p.error_expected("','");
    }
}

pub(crate) fn argument_list(p: &mut Parser) {
    p.start(ARGUMENT_LIST);
    p.expect(LPAREN, "'('");
    if p.at(ELLIPSIS) && p.nth(1) == RPAREN {
        p.bump();
    } else {
        comma_list(p, RPAREN, argument, |kind| {
            can_start_expr(kind) || matches!(kind, ELLIPSIS | QUESTION)
        });
    }
    p.expect(RPAREN, "')'");
    p.finish_node();
}

/// Whether the token after the cursor ends an argument, as it does after a placeholder.
fn ends_argument(p: &Parser, ahead: usize) -> bool {
    matches!(p.nth(ahead), COMMA | RPAREN)
}

fn argument(p: &mut Parser) -> bool {
    let named = is_identifier_like(p.current()) && p.nth(1) == COLON;
    let placeholder = p.at(QUESTION) && ends_argument(p, 1);
    if !named && !placeholder && !can_start_expr(p.current()) && !p.at(ELLIPSIS) {
        return false;
    }
    p.start(ARGUMENT);
    if placeholder {
        // `f(1, ?)`: partial application leaves the argument to the call of the closure it makes.
        p.bump();
    } else if p.at(ELLIPSIS) {
        p.bump();
        if !ends_argument(p, 0) {
            expr_required(p);
        }
    } else if named {
        p.bump();
        p.bump();
        if p.at(QUESTION) && ends_argument(p, 1) {
            p.bump();
        } else {
            expr_required(p);
        }
    } else {
        expr_required(p);
    }
    p.finish_node();
    true
}

fn array_expr(p: &mut Parser) -> SyntaxKind {
    p.start(ARRAY_EXPR);
    let close = if p.at(ARRAY_KW) {
        p.bump();
        p.bump();
        RPAREN
    } else {
        p.bump();
        RBRACKET
    };
    array_items(p, close);
    p.expect(close, if close == RPAREN { "')'" } else { "']'" });
    p.finish_node();
    ARRAY_EXPR
}

fn list_expr(p: &mut Parser) -> SyntaxKind {
    p.start(LIST_EXPR);
    p.bump();
    if p.expect(LPAREN, "'('") {
        array_items(p, RPAREN);
        p.expect(RPAREN, "')'");
    }
    p.finish_node();
    LIST_EXPR
}

/// Items of an array literal or a destructuring pattern, where an empty slot is allowed.
fn array_items(p: &mut Parser, close: SyntaxKind) {
    while !p.at(close) && !p.at_any(LIST_STOPS) {
        if p.eat(COMMA) {
            continue;
        }
        if !array_item(p) {
            p.error_bump();
            continue;
        }
        if p.eat(COMMA) {
            continue;
        }
        if p.at(close) || !(can_start_expr(p.current()) || matches!(p.current(), ELLIPSIS | AMP)) {
            break;
        }
        p.error_expected("','");
    }
}

fn array_item(p: &mut Parser) -> bool {
    if !can_start_expr(p.current()) && !matches!(p.current(), ELLIPSIS | AMP) {
        return false;
    }
    p.start(ARRAY_ITEM);
    match p.current() {
        ELLIPSIS | AMP => {
            p.bump();
            expr_required(p);
        }
        _ => {
            expr_required(p);
            if p.eat(FAT_ARROW) {
                p.eat(AMP);
                expr_required(p);
            }
        }
    }
    p.finish_node();
    true
}

fn match_expr(p: &mut Parser) -> SyntaxKind {
    p.start(MATCH_EXPR);
    p.bump();
    p.expect(LPAREN, "'('");
    expr_required(p);
    p.expect(RPAREN, "')'");
    if p.expect(LBRACE, "'{'") {
        while !p.at(RBRACE) && !p.eof() && !p.at_any(&[SEMICOLON, CLOSE_TAG]) {
            if !match_arm(p) {
                p.error_bump();
            }
            if !p.eat(COMMA) && !p.at(RBRACE) {
                break;
            }
        }
        p.expect(RBRACE, "'}'");
    }
    p.finish_node();
    MATCH_EXPR
}

fn match_arm(p: &mut Parser) -> bool {
    let is_default = p.at(DEFAULT_KW) && matches!(p.nth(1), FAT_ARROW | COMMA);
    if !is_default && !can_start_expr(p.current()) {
        return false;
    }
    p.start(MATCH_ARM);
    if is_default {
        p.bump();
        p.eat(COMMA);
    } else {
        loop {
            expr_required(p);
            if p.at(COMMA) && p.nth(1) != FAT_ARROW && can_start_expr(p.nth(1)) {
                p.bump();
            } else {
                p.eat(COMMA);
                break;
            }
        }
    }
    p.expect(FAT_ARROW, "'=>'");
    expr_required(p);
    p.finish_node();
    true
}

/// `function` and `fn`, with attributes and `static` before them already behind the checkpoint.
fn closure(p: &mut Parser, checkpoint: Checkpoint) -> SyntaxKind {
    let is_arrow = if p.at(STATIC_KW) {
        p.nth(1) == FN_KW
    } else {
        p.at(FN_KW)
    };
    let kind = if is_arrow { ARROW_FUNCTION_EXPR } else { CLOSURE_EXPR };
    p.start_at(checkpoint, kind);
    p.eat(STATIC_KW);
    p.bump();
    p.eat(AMP);
    item::parameter_list(p);
    if !is_arrow && p.at(USE_KW) {
        closure_use(p);
    }
    if p.at(COLON) {
        item::return_type(p);
    }
    if is_arrow {
        p.expect(FAT_ARROW, "'=>'");
        expr_required(p);
    } else if p.at(LBRACE) {
        stmt::block(p);
    } else {
        p.error_expected("'{'");
    }
    p.finish_node();
    kind
}

fn closure_use(p: &mut Parser) {
    p.start(CLOSURE_USE);
    p.bump();
    if p.expect(LPAREN, "'('") {
        comma_list(
            p,
            RPAREN,
            |p| {
                if !matches!(p.current(), VARIABLE | AMP) {
                    return false;
                }
                p.start(CLOSURE_USE_VARIABLE);
                p.eat(AMP);
                p.expect(VARIABLE, "Variable");
                p.finish_node();
                true
            },
            |kind| matches!(kind, VARIABLE | AMP),
        );
        p.expect(RPAREN, "')'");
    }
    p.finish_node();
}

/// `new` with its class and arguments. Says whether what follows may chain, which only holds
/// when the arguments or an anonymous class body close it.
fn new_expr(p: &mut Parser) -> bool {
    p.start(NEW_EXPR);
    p.bump();
    item::attribute_lists(p);
    let mut closed = false;
    if p.at(CLASS_KW) || (p.at(READONLY_KW) && p.nth(1) == CLASS_KW) {
        item::anonymous_class(p);
        closed = true;
    } else {
        class_name_reference(p);
        if p.at(LPAREN) {
            argument_list(p);
            closed = true;
        }
    }
    p.finish_node();
    closed
}

/// `new $class`, `new $a->b`, `new $a['x']`, `new $a::$b`: a variable with property and index
/// accesses but no calls.
fn new_variable(p: &mut Parser) {
    let checkpoint = p.checkpoint();
    variable(p);
    new_variable_tail(p, checkpoint);
}

fn new_variable_tail(p: &mut Parser, checkpoint: Checkpoint) {
    loop {
        match p.current() {
            LBRACKET => {
                p.start_at(checkpoint, INDEX_EXPR);
                p.bump();
                if !p.at(RBRACKET) {
                    expr_required(p);
                }
                p.expect(RBRACKET, "']'");
                p.finish_node();
            }
            ARROW | NULLSAFE_ARROW => {
                p.start_at(checkpoint, PROPERTY_FETCH_EXPR);
                p.bump();
                member_name(p, "Property name");
                p.finish_node();
            }
            DOUBLE_COLON if matches!(p.nth(1), VARIABLE | DOLLAR) => {
                p.start_at(checkpoint, STATIC_PROPERTY_EXPR);
                p.bump();
                variable(p);
                p.finish_node();
            }
            _ => break,
        }
    }
}
