//! PHPUnit: which classes and methods are tests, and the names tests give each other in strings.

pub mod complete;
mod detect;
pub mod mocks;
pub mod strings;

#[cfg(test)]
mod tests;

pub use detect::{ATTRIBUTES, TEST_CASE, has_attribute, is_test_class, is_test_method, test_case_ancestor};
