//! What a file says about tests that the declarations do not: the groups its classes belong to, the
//! Pest datasets it declares and the `uses()` and `pest()->extend()` calls that bind a test case to a
//! folder. They are read when a file is indexed so a question about another file does not read it.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode};
use serde::{Deserialize, Serialize};

use crate::model::Span;
use crate::resolve::NameResolver;
use crate::types::Name;

/// A `dataset('name', ...)` call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatasetDecl {
    pub name: String,
    /// The name inside its quotes.
    pub span: Span,
}

/// `uses(A::class, B::class)->in('Feature')` and `pest()->extend(A::class)->use(B::class)->in(...)`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestBinding {
    /// What the call names to bind: test case classes and traits, which the index tells apart.
    pub uses: Vec<Name>,
    /// The folders as written, relative to the file the call is in. Empty binds the file itself.
    pub folders: Vec<String>,
}

/// An `expect()->extend('name', fn)` call: an expectation a project adds to Pest's own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpectationDecl {
    pub name: String,
    /// The name inside its quotes.
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFacts {
    pub groups: Vec<String>,
    pub datasets: Vec<DatasetDecl>,
    pub bindings: Vec<TestBinding>,
    pub expectations: Vec<ExpectationDecl>,
}

impl TestFacts {
    pub fn is_empty(&self) -> bool {
        *self == TestFacts::default()
    }
}

/// One call of a chain such as `it('x', fn)->with('y')->group('z')`: the function that starts it,
/// then each method.
#[derive(Clone, Debug)]
pub struct ChainLink {
    /// As written: `it`, `pest`, `with`.
    pub name: String,
    pub arguments: Vec<SyntaxNode>,
    /// The call, or the property fetch for a link without a call.
    pub node: SyntaxNode,
}

/// The links of a chain from the function call it starts with, `None` when the expression is not
/// a chain of calls and property fetches on one.
pub fn flatten_chain(expression: &SyntaxNode) -> Option<Vec<ChainLink>> {
    let mut links = Vec::new();
    let mut current = expression.clone();
    loop {
        match current.kind() {
            CALL_EXPR => {
                let callee = current.children().next()?;
                let arguments = argument_expressions(&current);
                match callee.kind() {
                    NAME => {
                        links.push(ChainLink {
                            name: callee.text().to_string(),
                            arguments,
                            node: current,
                        });
                        break;
                    }
                    PROPERTY_FETCH_EXPR => {
                        let name = callee.children().find(|child| child.kind() == NAME)?;
                        links.push(ChainLink {
                            name: name.text().to_string(),
                            arguments,
                            node: current.clone(),
                        });
                        current = callee.children().next()?;
                    }
                    _ => return None,
                }
            }
            PROPERTY_FETCH_EXPR => {
                let name = current.children().find(|child| child.kind() == NAME)?;
                links.push(ChainLink {
                    name: name.text().to_string(),
                    arguments: Vec::new(),
                    node: current.clone(),
                });
                current = current.children().next()?;
            }
            _ => return None,
        }
    }
    links.reverse();
    Some(links)
}

/// The expressions a call is given, without the names of named arguments.
pub fn argument_expressions(call: &SyntaxNode) -> Vec<SyntaxNode> {
    call.children()
        .find(|child| child.kind() == ARGUMENT_LIST)
        .map(|list| {
            list.children()
                .filter(|child| child.kind() == ARGUMENT)
                .filter_map(|argument| argument.children().last())
                .collect()
        })
        .unwrap_or_default()
}

/// The text of a string literal without its quotes, and where that text is in the file.
pub fn string_value(node: &SyntaxNode) -> Option<(String, Span)> {
    if node.kind() != LITERAL {
        return None;
    }
    let token = node.children_with_tokens().find_map(|element| match element {
        SyntaxElement::Token(token) if token.kind() == STRING_LITERAL => Some(token),
        _ => None,
    })?;
    let text = token.text();
    let inner = text.get(1..text.len().checked_sub(1)?)?;
    let start = u32::from(token.text_range().start()) + 1;
    Some((
        inner.to_string(),
        Span {
            start,
            end: start + inner.len() as u32,
        },
    ))
}

/// The class `Foo::class` names, resolved.
pub fn class_constant(node: &SyntaxNode, resolver: &NameResolver) -> Option<Name> {
    if node.kind() != SCOPED_ACCESS_EXPR {
        return None;
    }
    let mut names = node.children().filter(|child| child.kind() == NAME);
    let (class, member) = (names.next()?, names.next()?);
    if !member.text().to_string().eq_ignore_ascii_case("class") {
        return None;
    }
    let raw = class.text().to_string();
    if matches!(raw.to_ascii_lowercase().as_str(), "self" | "static" | "parent") {
        return None;
    }
    Some(resolver.resolve_class(&raw))
}

/// A folder an `in()` call names: a string, `__DIR__` for the folder of the file, or `__DIR__`
/// followed by a path.
pub fn folder_of(argument: &SyntaxNode) -> Option<String> {
    if let Some((folder, _)) = string_value(argument) {
        return Some(folder);
    }
    let text = argument.text().to_string();
    if text.trim() == "__DIR__" {
        return Some(".".to_string());
    }
    if argument.kind() == BINARY_EXPR {
        let operands: Vec<SyntaxNode> = argument.children().collect();
        if let [left, right] = operands.as_slice()
            && left.text().to_string().trim() == "__DIR__"
            && let Some((rest, _)) = string_value(right)
        {
            return Some(rest.trim_start_matches('/').to_string());
        }
    }
    None
}

/// Reads one top-level statement of a file: a dataset, or a call that binds a test case.
pub fn read_statement(statement: &SyntaxNode, resolver: &NameResolver, facts: &mut TestFacts) {
    if statement.kind() != EXPR_STATEMENT {
        return;
    }
    let Some(expression) = statement.children().next() else {
        return;
    };
    let Some(links) = flatten_chain(&expression) else {
        return;
    };
    let root = links[0].name.trim_start_matches('\\').to_ascii_lowercase();
    match root.as_str() {
        "dataset" => {
            if let Some((name, span)) = links[0].arguments.first().and_then(string_value) {
                facts.datasets.push(DatasetDecl { name, span });
            }
        }
        "expect" => {
            if let Some(link) = links.get(1).filter(|link| link.name.eq_ignore_ascii_case("extend"))
                && let Some((name, span)) = link.arguments.first().and_then(string_value)
            {
                facts.expectations.push(ExpectationDecl { name, span });
            }
        }
        "uses" | "pest" => {
            let mut binding = TestBinding::default();
            for (position, link) in links.iter().enumerate() {
                let method = link.name.to_ascii_lowercase();
                match (position, method.as_str()) {
                    (0, "uses") | (_, "extend" | "use" | "uses") => {
                        for argument in &link.arguments {
                            if let Some(class) = class_constant(argument, resolver) {
                                binding.uses.push(class);
                            }
                        }
                    }
                    (_, "in") => {
                        for argument in &link.arguments {
                            if let Some(folder) = folder_of(argument) {
                                binding.folders.push(folder);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !binding.uses.is_empty() {
                facts.bindings.push(binding);
            }
        }
        _ => {}
    }
}

/// The names `#[Group('x')]` and `@group x` give inside a class.
pub fn read_groups(class: &SyntaxNode, resolver: &NameResolver, facts: &mut TestFacts) {
    for element in class.descendants_with_tokens() {
        match element {
            SyntaxElement::Node(node) if node.kind() == ATTRIBUTE => {
                let Some(name) = node.children().find(|child| child.kind() == NAME) else {
                    continue;
                };
                let resolved = resolver.resolve_class(&name.text().to_string());
                if !resolved.eq_ignore_ascii_case("PHPUnit\\Framework\\Attributes\\Group") {
                    continue;
                }
                if let Some((group, _)) = argument_expressions(&node).first().and_then(string_value) {
                    push_group(facts, group);
                }
            }
            SyntaxElement::Token(token) if token.kind() == DOC_COMMENT && token.text().contains("@group") => {
                for line in token.text().lines() {
                    let line = line.trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '/');
                    if let Some(rest) = line.strip_prefix("@group")
                        && let Some(group) = rest.split_whitespace().next()
                    {
                        push_group(facts, group.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

fn push_group(facts: &mut TestFacts, group: String) {
    if !facts.groups.contains(&group) {
        facts.groups.push(group);
    }
}

#[cfg(test)]
mod tests {
    use php_syntax::parse;

    use super::*;
    use crate::extract::{ExtractOptions, extract};

    fn facts(text: &str) -> TestFacts {
        extract(&parse(text).syntax(), ExtractOptions::default())
            .tests
            .map(|facts| *facts)
            .unwrap_or_default()
    }

    #[test]
    fn reads_datasets_and_bindings_of_a_pest_file() {
        let found = facts(
            "<?php\nnamespace Tests;\nuse Tests\\Support\\Case1;\nuse Lib\\Refresh;\ndataset('event sizes', [1, 2]);\nuses(Case1::class, Refresh::class)->in('Feature', 'Unit/Http');\npest()->extend(Case1::class)->use(Refresh::class)->in('Browser');\nuses(Case1::class);\n",
        );
        assert_eq!(found.datasets.len(), 1);
        assert_eq!(found.datasets[0].name, "event sizes");
        assert_eq!(found.bindings.len(), 3);
        assert_eq!(found.bindings[0].uses, vec!["Tests\\Support\\Case1", "Lib\\Refresh"]);
        assert_eq!(found.bindings[0].folders, vec!["Feature", "Unit/Http"]);
        assert_eq!(found.bindings[1].folders, vec!["Browser"]);
        assert!(found.bindings[2].folders.is_empty());
    }

    #[test]
    fn reads_the_groups_of_a_class() {
        let found = facts(
            "<?php\nuse PHPUnit\\Framework\\Attributes\\Group;\n#[Group('slow')]\nclass A {\n #[Group(\"db\")] function a() {}\n /** @group fast */ function b() {}\n}\n",
        );
        assert_eq!(found.groups, vec!["slow", "db", "fast"]);
    }

    #[test]
    fn reads_folders_written_from_dir_and_extended_expectations() {
        let found = facts(
            "<?php\nuses(A::class)->in(__DIR__);\nuses(B::class)->in(__DIR__ . '/Feature');\nexpect()->extend('toBeFoo', fn () => $this);\n",
        );
        assert_eq!(found.bindings[0].folders, vec!["."]);
        assert_eq!(found.bindings[1].folders, vec!["Feature"]);
        assert_eq!(found.expectations.len(), 1);
        assert_eq!(found.expectations[0].name, "toBeFoo");
    }

    #[test]
    fn a_file_with_nothing_to_say_holds_no_facts() {
        assert!(facts("<?php class A { function f() { return 1; } }").is_empty());
    }
}
