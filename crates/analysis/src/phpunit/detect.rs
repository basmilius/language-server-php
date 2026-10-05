//! Which classes are test cases and which of their methods are tests.

use php_index::{Attribute, ClassDecl, Index, Method, Visibility};

/// The class every PHPUnit test extends.
pub const TEST_CASE: &str = "PHPUnit\\Framework\\TestCase";

/// The namespace of PHPUnit's attributes.
pub const ATTRIBUTES: &str = "PHPUnit\\Framework\\Attributes\\";

/// Whether a class is a test case: it extends `TestCase`, directly or through classes of the
/// project. Without PHPUnit installed a parent named `TestCase` counts.
pub fn is_test_class(index: &Index, name: &str) -> bool {
    test_case_ancestor(index, name).is_some()
}

/// The `TestCase` a class reaches by its parents, or whatever stands in for it when the index has
/// no PHPUnit.
pub fn test_case_ancestor(index: &Index, name: &str) -> Option<String> {
    let mut current = name.trim_start_matches('\\').to_string();
    for _ in 0..32 {
        if current.eq_ignore_ascii_case(TEST_CASE) {
            return Some(current);
        }
        let Some(class) = index.class(&current) else {
            return crate::short(&current)
                .eq_ignore_ascii_case("TestCase")
                .then_some(current);
        };
        let parent = class
            .decl
            .extends
            .first()?
            .class_names()
            .into_iter()
            .next()?
            .to_string();
        current = parent;
    }
    None
}

/// Whether a list of attributes holds a PHPUnit attribute of this short name.
pub fn has_attribute(attributes: &[Attribute], short: &str) -> bool {
    attributes.iter().any(|attribute| {
        attribute
            .name
            .strip_prefix(ATTRIBUTES)
            .is_some_and(|name| name.eq_ignore_ascii_case(short))
    })
}

/// Whether a method of a test case is a test: public, with a name that starts with `test`, or marked
/// with `#[Test]` or `@test`.
pub fn is_test_method(class: &ClassDecl, method: &Method) -> bool {
    if method.visibility != Visibility::Public || method.is_static || method.is_abstract {
        return false;
    }
    if class.kind != php_index::ClassKind::Class {
        return false;
    }
    method.name.starts_with("test")
        || has_attribute(&method.attributes, "Test")
        || method
            .doc
            .as_ref()
            .is_some_and(|doc| doc.tags.iter().any(|tag| tag.name == "test"))
}
