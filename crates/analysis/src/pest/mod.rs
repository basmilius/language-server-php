//! Pest: test files made of function calls, the case class their closures run in and datasets.

pub mod calls;
pub mod case;
mod datasets;
mod expectations;

#[cfg(test)]
mod tests;

use php_syntax::SyntaxNode;

use crate::infer::Analyzer;
use crate::phpunit::strings::TestString;

pub use datasets::{dataset_candidates, dataset_declarations, dataset_hints};
pub use expectations::{CustomExpectation, custom_expectations, is_expectation};

/// The strings of a Pest call that name a dataset.
pub fn call_strings(_analyzer: &Analyzer<'_>, call: &SyntaxNode) -> Vec<TestString> {
    datasets::call_strings(call)
}
