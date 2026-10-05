//! Joining strings with `.`, writing them with variables inside, or with `sprintf`, and back.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode};

use super::{around, caret, rewrite, text_of_node};
use crate::actions::edits::replace;
use crate::ast::{start, text_of, tokens};
use crate::refactor::draft::Draft;
use crate::refactor::exprs::binary_operator;
use crate::refactor::signature::arguments_of;
use crate::refactor::{Rcx, Refactor};

/// A piece of a string as it will be written.
enum Piece {
    Text(String),
    /// An expression and the code that writes it.
    Value {
        code: String,
        plain_variable: bool,
        starts_with_variable: bool,
    },
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let nodes: Vec<SyntaxNode> = around(&token).collect();
    if let Some(chain) = nodes.iter().rfind(|node| is_concat(node)) {
        if let Ok(pieces) = pieces_of_concat(rcx, chain) {
            let chain = chain.clone();
            if as_interpolation(&pieces).is_ok() {
                let (id, node) = (format!("rewrite-interpolate@{}", start(&chain)), chain.clone());
                out.push(rewrite(
                    id,
                    "Convert concatenation to an interpolated string",
                    move || replace_with(rcx, &node, as_interpolation(&pieces_of_concat(rcx, &node)?)?),
                ));
            }
            let (id, node) = (format!("rewrite-sprintf@{}", start(&chain)), chain);
            out.push(rewrite(id, "Convert concatenation to sprintf", move || {
                replace_with(rcx, &node, as_sprintf(&pieces_of_concat(rcx, &node)?)?)
            }));
        }
    }
    if let Some(string) = nodes.iter().find(|node| node.kind() == INTERPOLATED_STRING) {
        if as_concatenation(&pieces_of_string(rcx, string).unwrap_or_default()).is_ok()
            && pieces_of_string(rcx, string).is_ok()
        {
            let node = string.clone();
            out.push(rewrite(
                format!("rewrite-concatenate@{}", start(string)),
                "Convert the string to a concatenation",
                move || replace_with(rcx, &node, as_concatenation(&pieces_of_string(rcx, &node)?)?),
            ));
        }
    }
    if let Some(call) = nodes.iter().find(|node| is_sprintf(node)) {
        if let Ok(pieces) = pieces_of_sprintf(rcx, call) {
            let call = call.clone();
            let (id, node) = (format!("rewrite-unsprintf@{}", start(&call)), call.clone());
            out.push(rewrite(id, "Convert sprintf to a concatenation", move || {
                replace_with(rcx, &node, as_concatenation(&pieces_of_sprintf(rcx, &node)?)?)
            }));
            if as_interpolation(&pieces).is_ok() {
                let (id, node) = (format!("rewrite-unsprintf-interpolate@{}", start(&call)), call);
                out.push(rewrite(id, "Convert sprintf to an interpolated string", move || {
                    replace_with(rcx, &node, as_interpolation(&pieces_of_sprintf(rcx, &node)?)?)
                }));
            }
        }
    }
}

fn replace_with(rcx: &Rcx<'_>, node: &SyntaxNode, text: String) -> Result<crate::refactor::Change, String> {
    let mut draft = Draft::new(rcx.renv);
    draft.here(replace(node.text_range(), text));
    draft.finish()
}

fn is_concat(node: &SyntaxNode) -> bool {
    node.kind() == BINARY_EXPR && binary_operator(node) == Some(DOT)
}

fn is_sprintf(node: &SyntaxNode) -> bool {
    node.kind() == CALL_EXPR
        && node.children().next().is_some_and(|callee| {
            callee.kind() == NAME
                && text_of(&callee)
                    .trim_start_matches('\\')
                    .eq_ignore_ascii_case("sprintf")
        })
}

// Reading strings -------------------------------------------------------------------------------

/// The characters a double-quoted string stands for, with its escapes undone.
fn decode_double(raw: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(next) = chars.next() else {
            out.push('\\');
            break;
        };
        match next {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'v' => out.push('\u{b}'),
            'e' => out.push('\u{1b}'),
            'f' => out.push('\u{c}'),
            '\\' => out.push('\\'),
            '$' => out.push('$'),
            '"' => out.push('"'),
            '0'..='7' => {
                let mut value = next.to_digit(8)?;
                for _ in 0..2 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(digit) => {
                            value = value * 8 + digit;
                            chars.next();
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(value)?);
            }
            'x' => {
                let mut value = 0;
                let mut count = 0;
                while count < 2 {
                    match chars.peek().and_then(|c| c.to_digit(16)) {
                        Some(digit) => {
                            value = value * 16 + digit;
                            chars.next();
                            count += 1;
                        }
                        None => break,
                    }
                }
                if count == 0 {
                    out.push_str("\\x");
                } else {
                    out.push(char::from_u32(value)?);
                }
            }
            'u' if chars.peek() == Some(&'{') => {
                chars.next();
                let mut digits = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    digits.push(c);
                }
                out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Some(out)
}

fn decode_single(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && matches!(chars.peek(), Some('\\' | '\'')) {
            out.extend(chars.next());
        } else {
            out.push(c);
        }
    }
    out
}

/// The text of a string literal node, with the quotes and escapes gone.
fn literal_text(node: &SyntaxNode) -> Option<String> {
    if node.kind() != LITERAL {
        return None;
    }
    let token = tokens(node).find(|token| token.kind() == STRING_LITERAL)?;
    let text = token.text();
    let inner = text.get(1..text.len().checked_sub(1)?)?;
    match text.chars().next()? {
        '\'' => Some(decode_single(inner)),
        '"' => decode_double(inner),
        _ => None,
    }
}

fn number_text(node: &SyntaxNode) -> Option<String> {
    if node.kind() != LITERAL {
        return None;
    }
    let token = tokens(node).find(|token| matches!(token.kind(), INT_LITERAL | FLOAT_LITERAL))?;
    let plain = token.text().chars().all(|c| c.is_ascii_digit() || c == '.');
    plain.then(|| token.text().to_string())
}

fn value_piece(rcx: &Rcx<'_>, node: &SyntaxNode) -> Piece {
    let code = text_of_node(rcx.cx.text, node).to_string();
    Piece::Value {
        plain_variable: node.kind() == VARIABLE_EXPR,
        starts_with_variable: code.starts_with('$')
            && matches!(
                node.kind(),
                VARIABLE_EXPR | PROPERTY_FETCH_EXPR | INDEX_EXPR | STATIC_PROPERTY_EXPR | CALL_EXPR
            ),
        code,
    }
}

fn flatten(node: &SyntaxNode, out: &mut Vec<SyntaxNode>) {
    if is_concat(node) {
        for operand in node.children() {
            flatten(&operand, out);
        }
    } else {
        out.push(node.clone());
    }
}

fn pieces_of_concat(rcx: &Rcx<'_>, chain: &SyntaxNode) -> Result<Vec<Piece>, String> {
    let mut operands = Vec::new();
    flatten(chain, &mut operands);
    let mut pieces: Vec<Piece> = Vec::new();
    for operand in &operands {
        if super::has_comment(operand) {
            return Err("There are comments in it".to_string());
        }
        if let Some(text) = literal_text(operand).or_else(|| number_text(operand)) {
            push_text(&mut pieces, text);
        } else {
            pieces.push(value_piece(rcx, operand));
        }
    }
    Ok(pieces)
}

fn push_text(pieces: &mut Vec<Piece>, text: String) {
    match pieces.last_mut() {
        Some(Piece::Text(held)) => held.push_str(&text),
        _ => pieces.push(Piece::Text(text)),
    }
}

fn pieces_of_string(rcx: &Rcx<'_>, string: &SyntaxNode) -> Result<Vec<Piece>, String> {
    let mut pieces: Vec<Piece> = Vec::new();
    for element in string.children_with_tokens() {
        match element {
            SyntaxElement::Token(token) => match token.kind() {
                STRING_CONTENT => push_text(
                    &mut pieces,
                    decode_double(token.text()).ok_or("An escape cannot be read")?,
                ),
                DOUBLE_QUOTE => {}
                _ => return Err("The string is not a plain interpolated string".to_string()),
            },
            SyntaxElement::Node(node) => match node.kind() {
                VARIABLE_EXPR => pieces.push(value_piece(rcx, &node)),
                PROPERTY_FETCH_EXPR => pieces.push(value_piece(rcx, &node)),
                INDEX_EXPR => {
                    let key = node.children().nth(1).ok_or("No key")?;
                    if !matches!(key.kind(), VARIABLE_EXPR) && number_text(&key).is_none() {
                        return Err("A bare key means a string".to_string());
                    }
                    pieces.push(value_piece(rcx, &node));
                }
                BRACED_INTERPOLATION => {
                    let inner = node.children().next().ok_or("Nothing in the braces")?;
                    pieces.push(value_piece(rcx, &inner));
                }
                _ => return Err("The string has a form that cannot be written otherwise".to_string()),
            },
        }
    }
    Ok(pieces)
}

fn pieces_of_sprintf(rcx: &Rcx<'_>, call: &SyntaxNode) -> Result<Vec<Piece>, String> {
    let given = arguments_of(call);
    if given.iter().any(|arg| arg.spread || arg.name.is_some()) {
        return Err("The call spreads or names its arguments".to_string());
    }
    let format = given
        .first()
        .and_then(|arg| arg.value.as_ref())
        .and_then(literal_text)
        .ok_or("The format is not a plain string")?;
    let mut values = given.iter().skip(1).filter_map(|arg| arg.value.clone());
    let mut pieces: Vec<Piece> = Vec::new();
    let mut chars = format.chars().peekable();
    let mut used = 0;
    while let Some(c) = chars.next() {
        if c != '%' {
            push_text(&mut pieces, c.to_string());
            continue;
        }
        match chars.next() {
            Some('%') => push_text(&mut pieces, "%".to_string()),
            Some('s') => {
                let value = values.next().ok_or("A placeholder has no argument")?;
                used += 1;
                match literal_text(&value) {
                    Some(text) => push_text(&mut pieces, text),
                    None => pieces.push(value_piece(rcx, &value)),
                }
            }
            _ => return Err("The format has more than plain %s".to_string()),
        }
    }
    if used != given.len().saturating_sub(1) {
        return Err("The arguments and the placeholders do not match".to_string());
    }
    Ok(pieces)
}

// Writing strings -------------------------------------------------------------------------------

fn needs_double(text: &str) -> bool {
    text.chars().any(|c| c.is_control() && c != '\n')
}

fn encode_single(text: &str) -> String {
    format!("'{}'", text.replace('\\', "\\\\").replace('\'', "\\'"))
}

fn encode_double_body(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{1b}' => out.push_str("\\e"),
            c if c.is_control() => out.push_str(&format!("\\x{:02X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A string literal that holds the text, quoted the plainest way that can hold it.
fn quoted(text: &str) -> String {
    if needs_double(text) {
        format!("\"{}\"", encode_double_body(text))
    } else {
        encode_single(text)
    }
}

fn as_interpolation(pieces: &[Piece]) -> Result<String, String> {
    if !pieces.iter().any(|piece| matches!(piece, Piece::Value { .. })) {
        return Err("There is nothing to put in the string".to_string());
    }
    let mut out = String::from("\"");
    for (position, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Text(text) => out.push_str(&encode_double_body(text)),
            Piece::Value {
                code,
                plain_variable,
                starts_with_variable,
            } => {
                if !starts_with_variable {
                    return Err("Only variables go inside a string".to_string());
                }
                let next = pieces.get(position + 1).and_then(|next| match next {
                    Piece::Text(text) => text.chars().next(),
                    Piece::Value { .. } => Some('{'),
                });
                let clashes = next.is_some_and(|c| c.is_alphanumeric() || matches!(c, '_' | '[' | '-' | '{' | '\\'));
                if *plain_variable && !clashes {
                    out.push_str(code);
                } else {
                    out.push_str(&format!("{{{code}}}"));
                }
            }
        }
    }
    out.push('"');
    Ok(out)
}

fn as_sprintf(pieces: &[Piece]) -> Result<String, String> {
    let mut format = String::new();
    let mut arguments: Vec<&str> = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) => format.push_str(&text.replace('%', "%%")),
            Piece::Value { code, .. } => {
                format.push_str("%s");
                arguments.push(code);
            }
        }
    }
    if arguments.is_empty() {
        return Err("There is nothing to put in".to_string());
    }
    Ok(format!("sprintf({}, {})", quoted(&format), arguments.join(", ")))
}

fn as_concatenation(pieces: &[Piece]) -> Result<String, String> {
    let mut parts: Vec<String> = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(text) if text.is_empty() => {}
            Piece::Text(text) => parts.push(quoted(text)),
            Piece::Value { code, .. } => parts.push(code.clone()),
        }
    }
    if pieces
        .iter()
        .filter(|piece| matches!(piece, Piece::Value { .. }))
        .count()
        == 0
    {
        return Err("There is nothing to join".to_string());
    }
    if parts.len() < 2 {
        return Err("A single value is not a concatenation".to_string());
    }
    Ok(parts.join(" . "))
}
