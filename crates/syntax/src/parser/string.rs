//! Interpolated strings, backtick strings and heredocs. The lexer has already split them into
//! text, variables and `{$` or `${` blocks; this only gives the pieces a shape.

use rowan::{TextRange, TextSize};

use super::Parser;
use super::expr::{self, name};
use crate::SyntaxKind::{self, *};

pub(crate) fn interpolated_string(p: &mut Parser) {
    p.start(INTERPOLATED_STRING);
    p.bump();
    parts(p, DOUBLE_QUOTE);
    close(p, DOUBLE_QUOTE, "'\"'");
    p.finish_node();
}

pub(crate) fn shell_exec(p: &mut Parser) {
    p.start(SHELL_EXEC_EXPR);
    p.bump();
    parts(p, BACKTICK);
    close(p, BACKTICK, "'`'");
    p.finish_node();
}

pub(crate) fn heredoc(p: &mut Parser) {
    let interpolates = !p.current_text().contains('\'');
    p.start(HEREDOC);
    p.bump();
    let mut texts = Vec::new();
    parts_in(p, HEREDOC_END, interpolates, Some(&mut texts));
    if p.at(HEREDOC_END) {
        check_indentation(p, &texts);
    }
    close(p, HEREDOC_END, "Heredoc closing marker");
    p.finish_node();
}

/// Checks the body of a heredoc against the indentation of its closing marker. `texts` are the
/// stretches of plain text with the offset each starts at; code inside `{$` blocks is not text.
fn check_indentation(p: &mut Parser, texts: &[(u32, &str)]) {
    let marker = p.current_text();
    let indentation = &marker[..marker.len() - marker.trim_start_matches([' ', '\t']).len()];
    if indentation.is_empty() {
        return;
    }
    let marker_start = p.current_offset();
    let uses_tabs = indentation.starts_with('\t');
    if indentation.contains(if uses_tabs { ' ' } else { '\t' }) {
        let range = TextRange::at(TextSize::from(marker_start), TextSize::of(indentation));
        p.error_at(range, "Invalid indentation - tabs and spaces cannot be mixed");
        return;
    }
    let indent_byte = indentation.as_bytes()[0];
    let required = indentation.len();
    for (offset, text) in texts {
        let bytes = text.as_bytes();
        let mut line_starts = Vec::new();
        if *offset == 0 || p.byte_before(*offset) == Some(b'\n') {
            line_starts.push(0);
        }
        for (index, byte) in bytes.iter().enumerate() {
            if *byte == b'\n' && index + 1 < bytes.len() {
                line_starts.push(index + 1);
            }
        }
        for start in line_starts {
            check_line(p, *offset, bytes, start, required, indent_byte);
        }
    }
}

fn check_line(p: &mut Parser, offset: u32, bytes: &[u8], start: usize, required: usize, indent_byte: u8) {
    for index in 0..required {
        match bytes.get(start + index) {
            Some(byte) if *byte == indent_byte => {}
            Some(b' ' | b'\t') => {
                let range = TextRange::at(TextSize::from(offset + start as u32), TextSize::from(index as u32 + 1));
                p.error_at(range, "Invalid indentation - tabs and spaces cannot be mixed");
                return;
            }
            Some(b'\n' | b'\r') => return,
            _ => {
                let range = TextRange::at(TextSize::from(offset + start as u32), TextSize::from(index as u32));
                p.error_at(
                    range,
                    format!("Invalid body indentation level (expecting an indentation level of at least {required})"),
                );
                return;
            }
        }
    }
}

/// Reports a `\u{...}` escape PHP would reject.
pub(crate) fn check_escapes(p: &mut Parser, offset: u32, text: &str) {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at + 1 < bytes.len() {
        if bytes[at] != b'\\' {
            at += 1;
            continue;
        }
        if bytes[at + 1] != b'u' || bytes.get(at + 2) != Some(&b'{') {
            at += 2;
            continue;
        }
        let digits_start = at + 3;
        let mut end = digits_start;
        while bytes.get(end).is_some_and(u8::is_ascii_hexdigit) {
            end += 1;
        }
        let range = |to: usize| TextRange::new(TextSize::from(offset + at as u32), TextSize::from(offset + to as u32));
        if bytes.get(end) != Some(&b'}') || end == digits_start {
            p.error_at(
                range((end + 1).min(bytes.len())),
                "Invalid UTF-8 codepoint escape sequence",
            );
        } else if u32::from_str_radix(&text[digits_start..end], 16).map_or(true, |value| value > 0x10FFFF) {
            p.error_at(
                range(end + 1),
                "Invalid UTF-8 codepoint escape sequence: Codepoint too large",
            );
        }
        at = end + 1;
    }
}

fn close(p: &mut Parser, kind: SyntaxKind, what: &str) {
    if !p.eat(kind) {
        p.error_expected(what);
    }
}

fn parts(p: &mut Parser, end: SyntaxKind) {
    parts_in(p, end, true, None);
}

fn parts_in<'a>(p: &mut Parser<'a>, end: SyntaxKind, escapes: bool, mut texts: Option<&mut Vec<(u32, &'a str)>>) {
    while !p.eof() && !p.at(end) {
        if p.current() != STRING_CONTENT {
            if let Some(texts) = texts.as_deref_mut() {
                texts.push((p.current_offset(), ""));
            }
        }
        match p.current() {
            STRING_CONTENT => {
                let (offset, text) = (p.current_offset(), p.current_text());
                if escapes {
                    check_escapes(p, offset, text);
                }
                if let Some(texts) = texts.as_deref_mut() {
                    texts.push((offset, text));
                }
                p.bump();
            }
            VARIABLE => simple_interpolation(p),
            CURLY_OPEN => {
                p.start(BRACED_INTERPOLATION);
                p.bump();
                if expr::expr(p).is_none() {
                    p.error_expected("Expression");
                }
                p.expect(RBRACE, "'}'");
                p.finish_node();
            }
            DOLLAR_OPEN_CURLY => dollar_brace(p),
            _ => p.error_bump(),
        }
    }
}

/// `$a`, `$a[1]`, `$a[key]`, `$a[$i]`, `$a->b` and `$a?->b` inside a string.
fn simple_interpolation(p: &mut Parser) {
    let checkpoint = p.checkpoint();
    p.start(VARIABLE_EXPR);
    p.bump();
    p.finish_node();
    match p.current() {
        LBRACKET => {
            p.start_at(checkpoint, INDEX_EXPR);
            p.bump();
            p.eat(MINUS);
            match p.current() {
                INT_LITERAL | IDENT => {
                    p.start(LITERAL);
                    p.bump();
                    p.finish_node();
                }
                VARIABLE => {
                    p.start(VARIABLE_EXPR);
                    p.bump();
                    p.finish_node();
                }
                _ => p.error_expected("Index"),
            }
            p.expect(RBRACKET, "']'");
            p.finish_node();
        }
        ARROW | NULLSAFE_ARROW => {
            p.start_at(checkpoint, PROPERTY_FETCH_EXPR);
            p.bump();
            if p.at(IDENT) {
                name(p);
            } else {
                p.error_expected("Property name");
            }
            p.finish_node();
        }
        _ => {}
    }
}

/// `${name}`, `${name[expr]}` and `${expr}`.
fn dollar_brace(p: &mut Parser) {
    p.start(DOLLAR_BRACE_INTERPOLATION);
    p.bump();
    if p.at(IDENT) && matches!(p.nth(1), LBRACKET | RBRACE) {
        name(p);
        if p.at(LBRACKET) {
            p.bump();
            if expr::expr(p).is_none() {
                p.error_expected("Expression");
            }
            p.expect(RBRACKET, "']'");
        }
    } else if expr::expr(p).is_none() {
        p.error_expected("Expression");
    }
    p.expect(RBRACE, "'}'");
    p.finish_node();
}
