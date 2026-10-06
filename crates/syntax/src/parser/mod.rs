//! A recursive descent parser with a Pratt expression parser. It always produces a tree that holds
//! every byte of the input: what it cannot make sense of ends up in `ERROR` nodes.

mod expr;
mod item;
mod stmt;
mod string;
mod ty;

use std::ops::{Deref, DerefMut};

use rowan::{TextRange, TextSize};

use crate::kind::{PhpLanguage, SyntaxKind};
use crate::lexer;
use SyntaxKind::*;

pub use lsc_syntax::SyntaxError;

/// The tree of a text and the syntax errors found while building it.
pub type Parse = lsc_syntax::Parse<PhpLanguage>;

/// Parses a text. Never fails: a broken file gives a tree with `ERROR` nodes and a list of errors.
pub fn parse(text: &str) -> Parse {
    let lexed = lexer::lex(text);
    let mut parser = Parser {
        inner: lsc_syntax::Parser::new(text, lexed.tokens),
        void_cast_allowed: false,
    };
    for error in &lexed.errors {
        parser.error_at(
            TextRange::new(TextSize::from(error.start), TextSize::from(error.end)),
            error.message,
        );
    }
    stmt::source_file(&mut parser);
    parser.inner.finish()
}

/// The shared parser with what only PHP's grammar asks of it.
pub(crate) struct Parser<'a> {
    inner: lsc_syntax::Parser<'a, PhpLanguage>,
    /// Set while the statement being parsed is allowed to start with a `(void)` cast.
    pub(crate) void_cast_allowed: bool,
}

impl<'a> Deref for Parser<'a> {
    type Target = lsc_syntax::Parser<'a, PhpLanguage>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Parser<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Parser<'_> {
    /// Whether the `n`th upcoming token is an identifier that spells `word`, in any case.
    pub(crate) fn nth_is_word(&self, n: usize, word: &str) -> bool {
        self.nth(n) == IDENT && self.nth_text(n).eq_ignore_ascii_case(word)
    }
}

pub(crate) fn is_name_token(kind: SyntaxKind) -> bool {
    matches!(kind, IDENT | QUALIFIED_NAME | FULLY_QUALIFIED_NAME | RELATIVE_NAME)
}

/// Tokens that can be the name of a member, a constant or a label: PHP allows reserved words there.
pub(crate) fn is_identifier_like(kind: SyntaxKind) -> bool {
    kind == IDENT || kind == MAGIC_CONSTANT || kind.is_keyword()
}
