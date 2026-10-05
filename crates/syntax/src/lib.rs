//! Lexer, error-tolerant parser and lossless syntax tree for PHP.

mod dump;
mod kind;
mod language_level;
pub mod lexer;
mod parser;

pub use dump::{DumpOptions, dump, dump_compact};
pub use kind::{PhpLanguage, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken};
pub use language_level::{FEATURES, Feature, LevelDiagnostic, LevelSeverity, PhpVersion, check_language_level};
pub use parser::{Parse, SyntaxError, parse};
pub use rowan::{Direction, TextRange, TextSize, TokenAtOffset, WalkEvent};

#[cfg(test)]
mod tests;
