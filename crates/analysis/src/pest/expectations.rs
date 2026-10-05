//! Expectations a project adds with `expect()->extend('name', fn)`.

use std::path::PathBuf;

use php_index::test_facts::{TestFacts, read_statement};
use php_index::{Index, Span, Type};
use php_syntax::SyntaxNode;

use super::calls::top_statements;
use crate::infer::Analyzer;

/// An expectation of the project, with the string that declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomExpectation {
    pub name: String,
    /// The file it is declared in, `None` for the file being read.
    pub path: Option<PathBuf>,
    pub span: Span,
}

/// Every expectation the project adds, and the ones this file adds.
pub fn custom_expectations(index: &Index, root: &SyntaxNode) -> Vec<CustomExpectation> {
    let mut found: Vec<CustomExpectation> = index
        .test_files()
        .flat_map(|(file, facts)| {
            facts.expectations.iter().map(|declared| CustomExpectation {
                name: declared.name.clone(),
                path: Some(file.path.clone()),
                span: declared.span,
            })
        })
        .collect();
    let mut own = TestFacts::default();
    let resolver = php_index::extract::resolver_at(root, u32::MAX);
    for statement in top_statements(root) {
        read_statement(&statement, &resolver, &mut own);
    }
    found.extend(own.expectations.into_iter().map(|declared| CustomExpectation {
        name: declared.name,
        path: None,
        span: declared.span,
    }));
    found
}

/// Whether a type is one of Pest's expectation classes, which is where custom expectations are called.
pub fn is_expectation(ty: &Type) -> bool {
    ty.members().iter().any(|member| match member {
        Type::Class { name, .. } => {
            name.starts_with("Pest\\")
                && name
                    .rsplit('\\')
                    .next()
                    .is_some_and(|short| short.ends_with("Expectation"))
        }
        _ => false,
    })
}

impl Analyzer<'_> {
    /// The custom expectation a method name stands for.
    pub(crate) fn custom_expectation(&self, name: &str) -> Option<CustomExpectation> {
        let cached = self.shared.expectations.borrow().clone();
        let list = match cached {
            Some((root, list)) if root == self.root => list,
            _ => {
                let list = std::rc::Rc::new(custom_expectations(self.index, &self.root));
                *self.shared.expectations.borrow_mut() = Some((self.root.clone(), list.clone()));
                list
            }
        };
        list.iter().find(|expectation| expectation.name == name).cloned()
    }
}
