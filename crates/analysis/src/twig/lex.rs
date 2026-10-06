//! The tokens of a Twig template, read the way Twig's lexer reads it: markup, `{{ }}` and `{% %}`
//! with the expressions in them, `{# #}` comments, and `{% verbatim %}` blocks left as markup.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    /// `{{`, with its `-` or `~`.
    VarStart,
    VarEnd,
    /// `{%`, with its `-` or `~`.
    BlockStart,
    BlockEnd,
    Name,
    Number,
    /// A string with its quotes.
    Str,
    /// Punctuation and operators that are not words: `.`, `|`, `(`, `==`, `..` and the like.
    Punct,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub start: u32,
    pub end: u32,
}

impl Token {
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start as usize..self.end as usize]
    }
}

const PUNCTUATION: &[&str] = &[
    "...", "<=>", "..", "**", "//", "==", "!=", "<=", ">=", "??", "?:", "=>", "?.", "+", "-", "*", "/", "%", "~", "<",
    ">", "=", "|", ".", ",", ":", "?", "(", ")", "[", "]", "{", "}",
];

fn is_name_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

pub fn lex(text: &str) -> Vec<Token> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        let Some(found) = next_delimiter(text, position) else {
            out.push(Token {
                kind: Kind::Text,
                start: position as u32,
                end: bytes.len() as u32,
            });
            break;
        };
        if found > position {
            out.push(Token {
                kind: Kind::Text,
                start: position as u32,
                end: found as u32,
            });
        }
        let opener = &text[found..found + 2];
        let modifier = usize::from(matches!(bytes.get(found + 2), Some(b'-' | b'~')));
        match opener {
            "{#" => {
                position = text[found + 2..]
                    .find("#}")
                    .map_or(bytes.len(), |end| found + 2 + end + 2);
            }
            "{{" => {
                out.push(Token {
                    kind: Kind::VarStart,
                    start: found as u32,
                    end: (found + 2 + modifier) as u32,
                });
                position = expression(text, found + 2 + modifier, "}}", Kind::VarEnd, &mut out);
            }
            _ => {
                out.push(Token {
                    kind: Kind::BlockStart,
                    start: found as u32,
                    end: (found + 2 + modifier) as u32,
                });
                let before = out.len();
                position = expression(text, found + 2 + modifier, "%}", Kind::BlockEnd, &mut out);
                let tag = out
                    .get(before)
                    .filter(|token| token.kind == Kind::Name)
                    .map(|token| token.text(text));
                if let Some(raw @ ("verbatim" | "raw")) = tag {
                    let end_tag = format!("end{raw}");
                    position = raw_block(text, position, &end_tag, &mut out);
                }
            }
        }
    }
    out
}

/// Where the next `{{`, `{%` or `{#` is.
fn next_delimiter(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut at = from;
    while let Some(found) = text.get(at..)?.find('{') {
        let open = at + found;
        if matches!(bytes.get(open + 1), Some(b'{' | b'%' | b'#')) {
            return Some(open);
        }
        at = open + 1;
    }
    None
}

/// Reads the tokens of an expression up to its closing delimiter and returns where reading goes on.
fn expression(text: &str, from: usize, close: &str, end_kind: Kind, out: &mut Vec<Token>) -> usize {
    let bytes = text.as_bytes();
    let mut position = from;
    while position < bytes.len() {
        let byte = bytes[position];
        if byte.is_ascii_whitespace() {
            position += 1;
            continue;
        }
        let rest = &text[position..];
        let modifier = usize::from(matches!(byte, b'-' | b'~') && rest[1..].starts_with(close));
        if rest[modifier..].starts_with(close) {
            out.push(Token {
                kind: end_kind,
                start: position as u32,
                end: (position + modifier + 2) as u32,
            });
            return position + modifier + 2;
        }
        let start = position;
        let kind = if is_name_start(byte) {
            position += rest.bytes().take_while(|byte| is_name_byte(*byte)).count();
            Kind::Name
        } else if byte.is_ascii_digit() {
            position += rest
                .bytes()
                .take_while(|byte| byte.is_ascii_digit() || *byte == b'_')
                .count();
            if bytes.get(position) == Some(&b'.') && bytes.get(position + 1).is_some_and(u8::is_ascii_digit) {
                position += 1;
                position += text[position..].bytes().take_while(u8::is_ascii_digit).count();
            }
            Kind::Number
        } else if byte == b'\'' || byte == b'"' {
            position += 1;
            while position < bytes.len() && bytes[position] != byte {
                position += if bytes[position] == b'\\' { 2 } else { 1 };
            }
            position = (position + 1).min(bytes.len());
            Kind::Str
        } else if let Some(punct) = PUNCTUATION.iter().find(|punct| rest.starts_with(**punct)) {
            position += punct.len();
            Kind::Punct
        } else {
            position += rest.chars().next().map_or(1, char::len_utf8);
            Kind::Punct
        };
        out.push(Token {
            kind,
            start: start as u32,
            end: position as u32,
        });
    }
    position
}

/// Skips the markup of a `{% verbatim %}` block up to its end tag, which is read as a tag.
fn raw_block(text: &str, from: usize, end_tag: &str, out: &mut Vec<Token>) -> usize {
    let mut at = from;
    while let Some(found) = text[at..].find("{%") {
        let open = at + found;
        let inner = text[open + 2..].trim_start_matches(['-', '~']).trim_start();
        if inner.starts_with(end_tag) {
            if open > from {
                out.push(Token {
                    kind: Kind::Text,
                    start: from as u32,
                    end: open as u32,
                });
            }
            return open;
        }
        at = open + 2;
    }
    out.push(Token {
        kind: Kind::Text,
        start: from as u32,
        end: text.len() as u32,
    });
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str) -> Vec<String> {
        lex(text)
            .iter()
            .map(|token| format!("{:?} {}", token.kind, token.text(text)))
            .collect()
    }

    #[test]
    fn reads_markup_output_tags_and_comments() {
        assert_eq!(
            shown("<p>{{ post.title|upper }}</p>{# c #}{%- if a == 1.5 -%}"),
            [
                "Text <p>",
                "VarStart {{",
                "Name post",
                "Punct .",
                "Name title",
                "Punct |",
                "Name upper",
                "VarEnd }}",
                "Text </p>",
                "BlockStart {%-",
                "Name if",
                "Name a",
                "Punct ==",
                "Number 1.5",
                "BlockEnd -%}",
            ]
        );
        assert_eq!(
            shown("{% verbatim %}{{ x }}{% endverbatim %}"),
            [
                "BlockStart {%",
                "Name verbatim",
                "BlockEnd %}",
                "Text {{ x }}",
                "BlockStart {%",
                "Name endverbatim",
                "BlockEnd %}",
            ]
        );
        assert_eq!(
            shown("{{ 'a }}' ~ \"b\" }}"),
            ["VarStart {{", "Str 'a }}'", "Punct ~", "Str \"b\"", "VarEnd }}"]
        );
        assert_eq!(
            shown("{% for i in 1..5 %}"),
            [
                "BlockStart {%",
                "Name for",
                "Name i",
                "Name in",
                "Number 1",
                "Punct ..",
                "Number 5",
                "BlockEnd %}"
            ]
        );
    }

    #[test]
    fn every_prefix_lexes() {
        let text = "{% extends 'a' %}{{ x.y('é')|f }}{# c #}{% verbatim %}{{ z {% endverbatim %}";
        for end in (0..=text.len()).filter(|end| text.is_char_boundary(*end)) {
            let _ = lex(&text[..end]);
        }
    }
}
