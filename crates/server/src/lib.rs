//! A PHP language server over LSP: document sync, diagnostics, symbols, folding and selection ranges,
//! and, over an index of the project, its packages and the standard library, hover, navigation,
//! workspace symbols and completion.

mod actions;
mod config;
mod convert;
mod documents;
mod features;
mod formatting;
mod hierarchies;
mod insight;
mod paths;
mod refactors;
mod runnables;
mod server;
mod usages;
mod workspace;

pub use server::run;
