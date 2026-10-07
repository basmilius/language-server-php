//! What a person writes to say a string is SQL: a `language=SQL` comment before the string or its
//! statement, or a heredoc whose label names SQL.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken};
use sql_embed::Dialect;

/// SQL, in the dialect the marker names, if it names one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Marker {
    pub dialect: Option<Dialect>,
}

/// What a language name says: SQL without a dialect, SQL of a dialect, or another language.
fn language(name: &str) -> Option<Option<Dialect>> {
    match name.to_ascii_lowercase().as_str() {
        "sql" | "genericsql" | "sql92" | "ansisql" => Some(None),
        "mysql" => Some(Some(Dialect::Mysql)),
        "mariadb" => Some(Some(Dialect::Mariadb)),
        "postgresql" | "postgres" | "pgsql" | "psql" => Some(Some(Dialect::Postgres)),
        "sqlite" | "sqlite3" => Some(Some(Dialect::Sqlite)),
        _ => None,
    }
}

/// The language a comment names with `language=`.
fn comment_marker(text: &str) -> Option<Marker> {
    let lower = text.to_ascii_lowercase();
    let at = lower.find("language=")?;
    let name: String = text[at + "language=".len()..]
        .chars()
        .take_while(|character| character.is_ascii_alphanumeric())
        .collect();
    language(&name).map(|dialect| Marker { dialect })
}

/// What a heredoc label says: `SQL`, or the name of a dialect.
pub(crate) fn label_marker(label: &str) -> Option<Marker> {
    language(label).map(|dialect| Marker { dialect })
}

fn is_comment(token: &SyntaxToken) -> bool {
    matches!(token.kind(), COMMENT | BLOCK_COMMENT | DOC_COMMENT)
}

/// A marker in the comments right before a node, or among the comments it starts with (a
/// declaration owns its doc comment).
pub(crate) fn before(node: &SyntaxNode) -> Option<Marker> {
    let first = node.first_token()?;
    let mut token = Some(first.clone());
    while let Some(current) = token {
        if current.kind() == WHITESPACE {
            token = current.next_token();
            continue;
        }
        if !is_comment(&current) || !node.text_range().contains_range(current.text_range()) {
            break;
        }
        if let Some(marker) = comment_marker(current.text()) {
            return Some(marker);
        }
        token = current.next_token();
    }
    let mut token = first.prev_token();
    while let Some(current) = token {
        match current.kind() {
            WHITESPACE => {}
            _ if is_comment(&current) => {
                if let Some(marker) = comment_marker(current.text()) {
                    return Some(marker);
                }
            }
            _ => break,
        }
        token = current.prev_token();
    }
    None
}

/// The statement or member a node is part of.
pub(crate) fn statement_of(node: &SyntaxNode) -> SyntaxNode {
    let mut current = node.clone();
    while let Some(parent) = current.parent() {
        if matches!(
            parent.kind(),
            BLOCK | STATEMENT_LIST | SOURCE_FILE | CASE_CLAUSE | DEFAULT_CLAUSE | CLASS_BODY
        ) {
            break;
        }
        current = parent;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_language_of_a_comment() {
        assert_eq!(comment_marker("/* language=SQL */"), Some(Marker { dialect: None }));
        assert_eq!(
            comment_marker("// Language=PostgreSQL"),
            Some(Marker {
                dialect: Some(Dialect::Postgres)
            })
        );
        assert_eq!(
            comment_marker("/** @lang x language=mariadb */"),
            Some(Marker {
                dialect: Some(Dialect::Mariadb)
            })
        );
        assert_eq!(comment_marker("// language=HTML"), None);
        assert_eq!(comment_marker("// select a file"), None);
        assert_eq!(label_marker("SQL"), Some(Marker { dialect: None }));
        assert_eq!(label_marker("HTML"), None);
    }
}
