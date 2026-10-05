//! The calls a Pest file is made of: `it()`, `test()`, `describe()`, the hooks and `dataset()`.

use php_index::test_facts::flatten_chain;
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

/// What a Pest function does, by the name it is called under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dsl {
    Test,
    It,
    Describe,
    BeforeEach,
    AfterEach,
    BeforeAll,
    AfterAll,
    Dataset,
    Arch,
    Todo,
    Uses,
    /// `expect()->extend('name', fn)`, whose closure runs with the expectation as `$this`.
    ExtendExpectation,
}

impl Dsl {
    /// The closure of this call runs with a test case as `$this`.
    pub fn binds_test_case(self) -> bool {
        matches!(
            self,
            Dsl::Test | Dsl::It | Dsl::BeforeEach | Dsl::AfterEach | Dsl::Arch | Dsl::ExtendExpectation
        )
    }

    /// The call declares a test that can be run.
    pub fn is_test(self) -> bool {
        matches!(self, Dsl::Test | Dsl::It | Dsl::Arch)
    }
}

/// The Pest function a name stands for. The namespace is ignored, so `\it` and `Pest\it` count.
pub fn dsl_of(raw: &str) -> Option<Dsl> {
    let name = raw.rsplit('\\').next().unwrap_or(raw);
    Some(match name.to_ascii_lowercase().as_str() {
        "test" => Dsl::Test,
        "it" => Dsl::It,
        "describe" => Dsl::Describe,
        "beforeeach" => Dsl::BeforeEach,
        "aftereach" => Dsl::AfterEach,
        "beforeall" => Dsl::BeforeAll,
        "afterall" => Dsl::AfterAll,
        "dataset" => Dsl::Dataset,
        "arch" => Dsl::Arch,
        "todo" => Dsl::Todo,
        "uses" => Dsl::Uses,
        _ => return None,
    })
}

/// The Pest call a closure is an argument of: `it('x', fn)`, `beforeEach(fn)`, and the hooks
/// called on `pest()` or `uses()`.
pub fn closure_owner(closure: &SyntaxNode) -> Option<(Dsl, SyntaxNode)> {
    if !matches!(closure.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR) {
        return None;
    }
    let call = closure
        .parent()
        .filter(|parent| parent.kind() == ARGUMENT)?
        .parent()
        .and_then(|list| list.parent())
        .filter(|call| call.kind() == CALL_EXPR)?;
    let callee = call.children().next()?;
    let name = match callee.kind() {
        NAME => callee.text().to_string(),
        PROPERTY_FETCH_EXPR => callee.children().find(|child| child.kind() == NAME)?.text().to_string(),
        _ => return None,
    };
    if callee.kind() == PROPERTY_FETCH_EXPR && name.eq_ignore_ascii_case("extend") {
        let chain = flatten_chain(&call)?;
        if chain[0].name.trim_start_matches('\\').eq_ignore_ascii_case("expect") {
            return Some((Dsl::ExtendExpectation, call));
        }
    }
    Some((dsl_of(&name)?, call))
}

/// The closure around a node that runs with a test case as `$this`, the innermost one that is the
/// body of a Pest test or hook.
pub fn enclosing_test_closure(node: &SyntaxNode) -> Option<SyntaxNode> {
    for ancestor in node.ancestors() {
        match ancestor.kind() {
            CLOSURE_EXPR | ARROW_FUNCTION_EXPR => {
                if let Some((dsl, _)) = closure_owner(&ancestor) {
                    return dsl.binds_test_case().then_some(ancestor);
                }
                if crate::ast::has_token(&ancestor, STATIC_KW) {
                    return None;
                }
            }
            kind if crate::ast::is_class_like(kind) || matches!(kind, FUNCTION_DECLARATION | METHOD_DECLARATION) => {
                return None;
            }
            _ => {}
        }
    }
    None
}

/// A Pest call that starts a statement of a body, with the links of its chain.
pub struct Statement {
    pub dsl: Dsl,
    pub chain: Vec<php_index::test_facts::ChainLink>,
    pub node: SyntaxNode,
}

impl Statement {
    /// The call that starts the chain.
    pub fn root(&self) -> &php_index::test_facts::ChainLink {
        &self.chain[0]
    }

    /// The description, when the first argument is a string.
    pub fn description(&self) -> Option<String> {
        php_index::test_facts::string_value(self.root().arguments.first()?).map(|(text, _)| text)
    }

    /// The closure of the call that starts the chain.
    pub fn closure(&self) -> Option<SyntaxNode> {
        self.root()
            .arguments
            .iter()
            .find(|argument| matches!(argument.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
            .cloned()
    }
}

/// The Pest statements directly in a container: a file, a namespace or the body of a closure.
pub fn statements_in(container: &SyntaxNode) -> Vec<Statement> {
    let mut out = Vec::new();
    for child in container.children() {
        match child.kind() {
            NAMESPACE_DECLARATION | STATEMENT_LIST | BLOCK => out.extend(statements_in(&child)),
            EXPR_STATEMENT => {
                let Some(expression) = child.children().next() else {
                    continue;
                };
                let Some(chain) = flatten_chain(&expression) else {
                    continue;
                };
                let Some(dsl) = dsl_of(&chain[0].name) else {
                    continue;
                };
                out.push(Statement {
                    dsl,
                    chain,
                    node: child,
                });
            }
            _ => {}
        }
    }
    out
}

/// The expression statements of a file that are not inside anything else.
pub fn top_statements(container: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut out = Vec::new();
    for child in container.children() {
        match child.kind() {
            NAMESPACE_DECLARATION | STATEMENT_LIST | BLOCK => out.extend(top_statements(&child)),
            EXPR_STATEMENT => out.push(child),
            _ => {}
        }
    }
    out
}

/// The body of a closure.
pub fn body_of(closure: &SyntaxNode) -> Option<SyntaxNode> {
    closure.children().find(|child| child.kind() == BLOCK)
}
