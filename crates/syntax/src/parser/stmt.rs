//! Statements, from the top of the file down to blocks, control structures and namespaces.

use rowan::{Checkpoint, TextRange, TextSize};

use super::expr::{self, can_start_expr, comma_list, expr_required, name};
use super::{Parser, is_identifier_like, is_name_token, item, ty};
use crate::SyntaxKind::{self, *};

/// Tokens that end a list of statements. `public` and its siblings are here because no statement
/// starts with them, and finding one inside a method body almost always means a `}` went missing.
const TERMINATORS: &[SyntaxKind] = &[
    RBRACE,
    ELSE_KW,
    ELSEIF_KW,
    ENDIF_KW,
    ENDWHILE_KW,
    ENDFOR_KW,
    ENDFOREACH_KW,
    ENDSWITCH_KW,
    ENDDECLARE_KW,
    CASE_KW,
    DEFAULT_KW,
    PUBLIC_KW,
    PROTECTED_KW,
    PRIVATE_KW,
];

pub(crate) fn source_file(p: &mut Parser) {
    p.start_root(SOURCE_FILE);
    top_level_statements(p, false);
    p.flush_rest();
    p.finish_node();
}

/// Statements of the file or of an unbraced namespace, where a token that closes something is an error.
fn top_level_statements(p: &mut Parser, in_namespace: bool) {
    while !p.eof() {
        if in_namespace && p.at(NAMESPACE_KW) {
            break;
        }
        let before = p.position();
        if p.at_any(TERMINATORS) {
            p.error_bump();
        } else {
            statement(p);
        }
        if p.position() == before {
            p.error_bump();
            if p.position() == before {
                break;
            }
        }
    }
}

/// Statements up to a token that ends the enclosing construct.
fn statement_list(p: &mut Parser) {
    while !p.eof() && !p.at_any(TERMINATORS) {
        let before = p.position();
        statement(p);
        if p.position() == before {
            p.error_bump();
            if p.position() == before {
                break;
            }
        }
    }
}

pub(crate) fn semicolon(p: &mut Parser) {
    if !p.eat(SEMICOLON) && !p.eat(CLOSE_TAG) {
        p.error_expected("';'");
    }
}

pub(crate) fn block(p: &mut Parser) {
    p.start(BLOCK);
    p.bump();
    statement_list(p);
    p.expect(RBRACE, "'}'");
    p.finish_node();
}

#[derive(Clone, Copy)]
enum Declaration {
    Function,
    ClassLike,
    Const,
}

/// What declaration starts `n` tokens ahead, if any.
fn declaration_at(p: &Parser, n: usize) -> Option<Declaration> {
    match p.nth(n) {
        FUNCTION_KW => {
            let name_at = if p.nth(n + 1) == AMP { n + 2 } else { n + 1 };
            is_identifier_like(p.nth(name_at)).then_some(Declaration::Function)
        }
        CONST_KW => Some(Declaration::Const),
        CLASS_KW | INTERFACE_KW | TRAIT_KW => Some(Declaration::ClassLike),
        ABSTRACT_KW | FINAL_KW | READONLY_KW => {
            let mut at = n;
            while matches!(p.nth(at), ABSTRACT_KW | FINAL_KW | READONLY_KW) {
                at += 1;
            }
            matches!(p.nth(at), CLASS_KW).then_some(Declaration::ClassLike)
        }
        IDENT
            if p.nth_is_word(n, "enum")
                && p.nth(n + 1) == IDENT
                && !p.nth_is_word(n + 1, "extends")
                && !p.nth_is_word(n + 1, "implements") =>
        {
            Some(Declaration::ClassLike)
        }
        _ => None,
    }
}

/// How many tokens the attribute lists at the cursor take.
fn attributes_len(p: &Parser) -> usize {
    let mut n = 0;
    while p.nth(n) == HASH_BRACKET {
        let mut depth = 0u32;
        loop {
            match p.nth(n) {
                EOF => return n,
                HASH_BRACKET | LBRACKET => depth += 1,
                RBRACKET => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        n += 1;
                        break;
                    }
                }
                _ => {}
            }
            n += 1;
        }
    }
    n
}

pub(crate) fn statement(p: &mut Parser) {
    if !p.enter() {
        p.error_bump();
        return;
    }
    statement_inner(p);
    p.leave();
}

fn statement_inner(p: &mut Parser) {
    match p.current() {
        OPEN_TAG | INLINE_HTML | CLOSE_TAG => p.bump(),
        OPEN_TAG_ECHO => echo_statement(p),
        SEMICOLON => {
            p.start(EMPTY_STATEMENT);
            p.bump();
            p.finish_node();
        }
        LBRACE => block(p),
        IF_KW => if_statement(p),
        WHILE_KW => while_statement(p),
        DO_KW => do_while_statement(p),
        FOR_KW => for_statement(p),
        FOREACH_KW => foreach_statement(p),
        SWITCH_KW => switch_statement(p),
        ECHO_KW => echo_statement(p),
        BREAK_KW => jump_statement(p, BREAK_STATEMENT),
        CONTINUE_KW => jump_statement(p, CONTINUE_STATEMENT),
        RETURN_KW => return_statement(p),
        GLOBAL_KW => global_statement(p),
        STATIC_KW if p.nth(1) == VARIABLE => static_variable_statement(p),
        UNSET_KW => unset_statement(p),
        TRY_KW => try_statement(p),
        GOTO_KW => goto_statement(p),
        DECLARE_KW => declare_statement(p),
        NAMESPACE_KW => namespace_declaration(p),
        USE_KW => use_statement(p),
        HALT_COMPILER_KW => halt_compiler_statement(p),
        HASH_BRACKET => {
            let n = attributes_len(p);
            if let Some(declaration) = declaration_at(p, n) {
                let checkpoint = p.declaration_checkpoint();
                item::attribute_lists(p);
                declaration_from(p, checkpoint, declaration);
            } else {
                expression_statement(p);
            }
        }
        IDENT if p.nth(1) == COLON => label_statement(p),
        _ => match declaration_at(p, 0) {
            Some(declaration) => {
                let checkpoint = p.declaration_checkpoint();
                declaration_from(p, checkpoint, declaration);
            }
            None => expression_statement(p),
        },
    }
}

fn declaration_from(p: &mut Parser, checkpoint: Checkpoint, declaration: Declaration) {
    match declaration {
        Declaration::Function => item::function_declaration(p, checkpoint),
        Declaration::ClassLike => item::class_like(p, checkpoint),
        Declaration::Const => const_statement(p, checkpoint),
    }
}

fn const_statement(p: &mut Parser, checkpoint: Checkpoint) {
    p.start_at(checkpoint, CONST_STATEMENT);
    p.bump();
    item::const_elements(p, true);
    semicolon(p);
    p.finish_node();
}

fn expression_statement(p: &mut Parser) {
    if !can_start_expr(p.current()) {
        p.error_bump();
        return;
    }
    p.start(EXPR_STATEMENT);
    p.void_cast_allowed = p.at(CAST);
    expr::expr(p);
    p.void_cast_allowed = false;
    semicolon(p);
    p.finish_node();
}

fn echo_statement(p: &mut Parser) {
    p.start(ECHO_STATEMENT);
    p.bump();
    comma_list(p, SEMICOLON, |p| expr::expr(p).is_some(), can_start_expr);
    semicolon(p);
    p.finish_node();
}

/// The body of a control structure that is not in alternative syntax: one statement.
fn body(p: &mut Parser) {
    if p.eof() || p.at_any(TERMINATORS) {
        p.error_expected("Statement");
        return;
    }
    statement(p);
}

/// `( expr )` after `if`, `while` and the like.
fn condition(p: &mut Parser) {
    if p.expect(LPAREN, "'('") {
        expr_required(p);
        p.expect(RPAREN, "')'");
    }
}

/// Either one statement or, after a colon, a list that ends at one of the `end...` keywords.
fn body_or_alternative(p: &mut Parser) -> bool {
    if p.at(COLON) {
        p.bump();
        p.start_before_declaration(STATEMENT_LIST);
        statement_list(p);
        p.finish_node();
        true
    } else {
        body(p);
        false
    }
}

fn end_keyword(p: &mut Parser, kind: SyntaxKind, what: &str) {
    p.expect(kind, what);
    semicolon(p);
}

fn if_statement(p: &mut Parser) {
    p.start(IF_STATEMENT);
    p.bump();
    condition(p);
    let alternative = body_or_alternative(p);
    while p.at_any(&[ELSEIF_KW, ELSE_KW]) {
        let is_else = p.at(ELSE_KW);
        p.start(if is_else { ELSE_CLAUSE } else { ELSEIF_CLAUSE });
        p.bump();
        if !is_else {
            condition(p);
        }
        body_or_alternative(p);
        p.finish_node();
    }
    if alternative {
        end_keyword(p, ENDIF_KW, "'endif'");
    }
    p.finish_node();
}

fn while_statement(p: &mut Parser) {
    p.start(WHILE_STATEMENT);
    p.bump();
    condition(p);
    if body_or_alternative(p) {
        end_keyword(p, ENDWHILE_KW, "'endwhile'");
    }
    p.finish_node();
}

fn do_while_statement(p: &mut Parser) {
    p.start(DO_WHILE_STATEMENT);
    p.bump();
    body(p);
    if p.expect(WHILE_KW, "'while'") {
        condition(p);
    }
    semicolon(p);
    p.finish_node();
}

fn for_statement(p: &mut Parser) {
    p.start(FOR_STATEMENT);
    p.bump();
    if p.expect(LPAREN, "'('") {
        for (index, close) in [SEMICOLON, SEMICOLON, RPAREN].into_iter().enumerate() {
            comma_list(
                p,
                close,
                |p| {
                    p.void_cast_allowed = p.at(CAST);
                    let parsed = expr::expr(p).is_some();
                    p.void_cast_allowed = false;
                    parsed
                },
                can_start_expr,
            );
            if index < 2 {
                if !p.eat(SEMICOLON) {
                    p.error_expected("';'");
                    break;
                }
            } else {
                p.expect(RPAREN, "')'");
            }
        }
    }
    if body_or_alternative(p) {
        end_keyword(p, ENDFOR_KW, "'endfor'");
    }
    p.finish_node();
}

fn foreach_statement(p: &mut Parser) {
    p.start(FOREACH_STATEMENT);
    p.bump();
    if p.expect(LPAREN, "'('") {
        expr_required(p);
        if p.expect(AS_KW, "'as'") {
            p.eat(AMP);
            expr_required(p);
            if p.eat(FAT_ARROW) {
                p.eat(AMP);
                expr_required(p);
            }
        }
        p.expect(RPAREN, "')'");
    }
    if body_or_alternative(p) {
        end_keyword(p, ENDFOREACH_KW, "'endforeach'");
    }
    p.finish_node();
}

fn switch_statement(p: &mut Parser) {
    p.start(SWITCH_STATEMENT);
    p.bump();
    condition(p);
    let alternative = p.at(COLON);
    if p.at_any(&[LBRACE, COLON]) {
        p.bump();
        while !p.eof() && !p.at_any(&[RBRACE, ENDSWITCH_KW]) {
            let before = p.position();
            match p.current() {
                CASE_KW => case_clause(p),
                DEFAULT_KW => default_clause(p),
                _ => p.error_bump(),
            }
            if p.position() == before {
                break;
            }
        }
        if alternative {
            end_keyword(p, ENDSWITCH_KW, "'endswitch'");
        } else {
            p.expect(RBRACE, "'}'");
        }
    } else {
        p.error_expected("'{'");
    }
    p.finish_node();
}

fn case_clause(p: &mut Parser) {
    p.start(CASE_CLAUSE);
    p.bump();
    expr_required(p);
    if !p.eat(COLON) && !p.eat(SEMICOLON) {
        p.error_expected("':'");
    }
    statement_list(p);
    p.finish_node();
}

fn default_clause(p: &mut Parser) {
    p.start(DEFAULT_CLAUSE);
    p.bump();
    if !p.eat(COLON) && !p.eat(SEMICOLON) {
        p.error_expected("':'");
    }
    statement_list(p);
    p.finish_node();
}

fn jump_statement(p: &mut Parser, kind: SyntaxKind) {
    p.start(kind);
    p.bump();
    if can_start_expr(p.current()) {
        expr::expr(p);
    }
    semicolon(p);
    p.finish_node();
}

fn return_statement(p: &mut Parser) {
    jump_statement(p, RETURN_STATEMENT);
}

fn global_statement(p: &mut Parser) {
    p.start(GLOBAL_STATEMENT);
    p.bump();
    comma_list(
        p,
        SEMICOLON,
        |p| {
            if !matches!(p.current(), VARIABLE | DOLLAR) {
                return false;
            }
            expr::variable(p);
            true
        },
        |kind| matches!(kind, VARIABLE | DOLLAR),
    );
    semicolon(p);
    p.finish_node();
}

fn static_variable_statement(p: &mut Parser) {
    p.start(STATIC_VARIABLE_STATEMENT);
    p.bump();
    comma_list(
        p,
        SEMICOLON,
        |p| {
            if !p.at(VARIABLE) {
                return false;
            }
            p.start(STATIC_VARIABLE);
            p.bump();
            if p.eat(ASSIGN) {
                expr_required(p);
            }
            p.finish_node();
            true
        },
        |kind| kind == VARIABLE,
    );
    semicolon(p);
    p.finish_node();
}

fn unset_statement(p: &mut Parser) {
    p.start(UNSET_STATEMENT);
    p.bump();
    if p.expect(LPAREN, "'('") {
        comma_list(
            p,
            RPAREN,
            |p| {
                let start = p.current_offset();
                match expr::expr(p) {
                    Some(kind) if expr::is_assignable(kind) => true,
                    Some(_) => {
                        let end = p.current_offset();
                        p.error_at(
                            TextRange::new(TextSize::from(start), TextSize::from(end)),
                            "Variable expected",
                        );
                        true
                    }
                    None => false,
                }
            },
            can_start_expr,
        );
        p.expect(RPAREN, "')'");
    }
    semicolon(p);
    p.finish_node();
}

fn try_statement(p: &mut Parser) {
    p.start(TRY_STATEMENT);
    p.bump();
    if p.at(LBRACE) {
        block(p);
    } else {
        p.error_expected("'{'");
    }
    while p.at(CATCH_KW) {
        p.start(CATCH_CLAUSE);
        p.bump();
        if p.expect(LPAREN, "'('") {
            ty::type_(p);
            p.eat(VARIABLE);
            p.expect(RPAREN, "')'");
        }
        if p.at(LBRACE) {
            block(p);
        } else {
            p.error_expected("'{'");
        }
        p.finish_node();
    }
    if p.at(FINALLY_KW) {
        p.start(FINALLY_CLAUSE);
        p.bump();
        if p.at(LBRACE) {
            block(p);
        } else {
            p.error_expected("'{'");
        }
        p.finish_node();
    }
    p.finish_node();
}

fn goto_statement(p: &mut Parser) {
    p.start(GOTO_STATEMENT);
    p.bump();
    if is_identifier_like(p.current()) {
        name(p);
    } else {
        p.error_expected("Label");
    }
    semicolon(p);
    p.finish_node();
}

fn label_statement(p: &mut Parser) {
    p.start(LABEL_STATEMENT);
    name(p);
    p.bump();
    p.finish_node();
}

fn declare_statement(p: &mut Parser) {
    p.start(DECLARE_STATEMENT);
    p.bump();
    if p.expect(LPAREN, "'('") {
        comma_list(
            p,
            RPAREN,
            |p| {
                if !is_identifier_like(p.current()) {
                    return false;
                }
                p.start(DECLARE_DIRECTIVE);
                name(p);
                p.expect(ASSIGN, "'='");
                expr_required(p);
                p.finish_node();
                true
            },
            is_identifier_like,
        );
        p.expect(RPAREN, "')'");
    }
    match p.current() {
        SEMICOLON | CLOSE_TAG => p.bump(),
        COLON => {
            p.bump();
            p.start_before_declaration(STATEMENT_LIST);
            statement_list(p);
            p.finish_node();
            end_keyword(p, ENDDECLARE_KW, "'enddeclare'");
        }
        _ => body(p),
    }
    p.finish_node();
}

fn namespace_declaration(p: &mut Parser) {
    p.start(NAMESPACE_DECLARATION);
    p.bump();
    if matches!(p.current(), FULLY_QUALIFIED_NAME | RELATIVE_NAME) {
        p.error_here("A namespace name cannot start with a backslash or 'namespace\\'");
    }
    if is_name_token(p.current()) || is_identifier_like(p.current()) {
        name(p);
    }
    if p.at(LBRACE) {
        block(p);
    } else {
        semicolon(p);
        p.start_before_declaration(STATEMENT_LIST);
        top_level_statements(p, true);
        p.finish_node();
    }
    p.finish_node();
}

fn use_statement(p: &mut Parser) {
    p.start(USE_STATEMENT);
    p.bump();
    let typed = matches!(p.current(), FUNCTION_KW | CONST_KW);
    p.eat_use_kind();
    if is_name_token(p.current()) && p.nth(1) == BACKSLASH {
        p.start(USE_GROUP);
        name(p);
        p.bump();
        if p.expect(LBRACE, "'{'") {
            if p.at(RBRACE) {
                p.error_expected("Name");
            }
            comma_list(
                p,
                RBRACE,
                |p| use_clause(p, true, typed),
                |kind| is_name_token(kind) || matches!(kind, FUNCTION_KW | CONST_KW),
            );
            p.expect(RBRACE, "'}'");
        }
        p.finish_node();
    } else {
        comma_list(p, SEMICOLON, |p| use_clause(p, false, false), is_name_token);
    }
    semicolon(p);
    p.finish_node();
}

fn use_clause(p: &mut Parser, in_group: bool, group_is_typed: bool) -> bool {
    if !is_name_token(p.current()) && !matches!(p.current(), FUNCTION_KW | CONST_KW) {
        return false;
    }
    p.start(USE_CLAUSE);
    if group_is_typed && matches!(p.current(), FUNCTION_KW | CONST_KW) {
        p.error_here("A group use cannot mix `function` or `const` with its own kind");
    }
    p.eat_use_kind();
    if in_group && matches!(p.current(), FULLY_QUALIFIED_NAME | RELATIVE_NAME) {
        p.error_here("A name in a group use cannot start with a backslash");
    }
    if is_name_token(p.current()) {
        name(p);
    } else {
        p.error_expected("Name");
    }
    if p.eat(AS_KW) {
        if is_identifier_like(p.current()) {
            name(p);
        } else {
            p.error_expected("Alias");
        }
    }
    p.finish_node();
    true
}

fn halt_compiler_statement(p: &mut Parser) {
    p.start(HALT_COMPILER_STATEMENT);
    p.bump();
    p.expect(LPAREN, "'('");
    p.expect(RPAREN, "')'");
    semicolon(p);
    p.eat(HALT_DATA);
    p.finish_node();
}

impl Parser<'_> {
    /// The `function` or `const` after `use`.
    fn eat_use_kind(&mut self) {
        if matches!(self.current(), FUNCTION_KW | CONST_KW) {
            self.bump();
        }
    }
}
