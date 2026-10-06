//! Questions about a syntax tree and an index that the language server answers: where the symbols
//! are, what folds, how a selection grows, which diagnostics a file has, what a variable is, what
//! completes at a cursor and what a name refers to. Nothing here knows about LSP or about processes,
//! so the same functions serve any front end.

pub mod actions;
mod ast;
pub mod blade;
pub mod class_strings;
pub mod completion;
pub mod context;
pub mod decl;
mod diagnostics;
pub mod doc_refs;
pub mod document;
mod folding;
pub mod frameworks;
pub mod hierarchy;
mod imports;
pub mod infer;
pub mod inlay_hints;
pub mod inspections;
pub mod nav;
pub mod pest;
pub mod phpunit;
pub mod refactor;
pub mod references;
pub mod refs;
pub mod rename;
pub mod render;
pub mod runnables;
mod selection;
pub mod semantic_tokens;
pub mod signature;
mod symbols;
pub mod target;
pub mod twig;
pub mod workspace_symbols;
pub mod yaml;

pub use diagnostics::{Diagnostic, DiagnosticSeverity, diagnostics};
pub use folding::{Fold, FoldKind, folding_ranges};
pub use infer::{Analyzer, Env};
pub use lsc_text::{LineCol, LineIndex, PositionEncoding};
pub use selection::selection_ranges;
pub use symbols::{Symbol, SymbolKind, document_symbols};

/// The last segment of a qualified name.
pub(crate) fn short(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

#[cfg(test)]
mod testing;

#[cfg(test)]
mod completion_tests;

#[cfg(test)]
mod nav_tests;

#[cfg(test)]
mod references_tests;

#[cfg(test)]
mod rename_tests;

#[cfg(test)]
mod signature_tests;

#[cfg(test)]
mod hierarchy_tests;

#[cfg(test)]
mod semantic_tokens_tests;

#[cfg(test)]
mod inlay_hints_tests;

#[cfg(test)]
mod robustness_tests;
