//! The test case a Pest closure runs in: the class and traits that `uses()` and `pest()->extend()`
//! bind to the folder of the file, and the properties `beforeEach` puts on it.

use std::path::Path;

use php_index::test_facts::{TestBinding, TestFacts, read_statement};
use php_index::{ClassKind, Index, Name, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::calls::{Dsl, body_of, closure_owner, enclosing_test_closure, statements_in, top_statements};
use crate::infer::{Analyzer, ClassContext};

/// The class a test runs in and the traits mixed into it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestCaseBinding {
    pub class: Name,
    pub traits: Vec<Name>,
}

/// Where a Pest closure is: the closure and what it runs as.
#[derive(Clone, Debug)]
pub struct PestScope {
    pub closure: SyntaxNode,
    pub binding: TestCaseBinding,
    /// The closure is the body of a custom expectation, where `$this` is the expectation.
    pub expectation: bool,
}

impl PestScope {
    /// The type `$this` has: the case class, with the traits bound to it.
    pub fn this_type(&self) -> Type {
        if self.expectation {
            return Type::Class {
                name: self.binding.class.clone(),
                args: vec![Type::Mixed],
            };
        }
        if self.binding.traits.is_empty() {
            return Type::class(self.binding.class.clone());
        }
        let mut parts = vec![Type::class(self.binding.class.clone())];
        parts.extend(self.binding.traits.iter().cloned().map(Type::class));
        Type::Intersection(parts)
    }

    pub fn class_context(&self) -> ClassContext {
        ClassContext {
            name: self.binding.class.clone(),
            parent: None,
            kind: ClassKind::Class,
            anonymous: false,
        }
    }
}

/// The scope of a Pest test at an offset, `None` outside the closure of a test or hook.
pub fn scope_at(
    index: &Index,
    root: &SyntaxNode,
    offset: u32,
    path: Option<&Path>,
    shared: &crate::infer::SharedCache,
) -> Option<PestScope> {
    let node = crate::ast::node_at(root, offset);
    let closure = enclosing_test_closure(&node)?;
    if matches!(closure_owner(&closure), Some((Dsl::ExtendExpectation, _))) {
        let class = index.class("Pest\\Expectation")?.decl.name.clone();
        return Some(PestScope {
            closure,
            binding: TestCaseBinding {
                class,
                traits: Vec::new(),
            },
            expectation: true,
        });
    }
    let cached = shared.pest_case.borrow().clone();
    let binding = match cached {
        Some(binding) => binding,
        None => {
            let binding = bound_case(index, root, path);
            *shared.pest_case.borrow_mut() = Some(binding.clone());
            binding
        }
    }?;
    Some(PestScope {
        closure,
        binding,
        expectation: false,
    })
}

/// What binds the test case to a file: `uses()` calls in the file itself and the ones in other files
/// (`tests/Pest.php`) that name a folder the file is in. The deepest folder names the class.
pub fn bound_case(index: &Index, root: &SyntaxNode, path: Option<&Path>) -> Option<TestCaseBinding> {
    let mut applying: Vec<(usize, Vec<Name>)> = Vec::new();
    let mut own = TestFacts::default();
    let resolver = php_index::extract::resolver_at(root, u32::MAX);
    for statement in top_statements(root) {
        read_statement(&statement, &resolver, &mut own);
    }
    for binding in &own.bindings {
        if binding.folders.is_empty() || path.is_some_and(|path| matches_folder(binding, path, path)) {
            applying.push((usize::MAX, binding.uses.clone()));
        }
    }
    if let Some(path) = path {
        for (file, facts) in index.test_files() {
            let Some(base) = file.path.parent() else {
                continue;
            };
            for binding in facts.bindings.iter().filter(|binding| !binding.folders.is_empty()) {
                if let Some(depth) = deepest_match(binding, base, path) {
                    applying.push((depth, binding.uses.clone()));
                }
            }
        }
    }
    applying.sort_by_key(|(depth, _)| *depth);
    let mut class = None;
    let mut traits: Vec<Name> = Vec::new();
    for (_, names) in &applying {
        for name in names {
            let is_trait = index
                .class(name)
                .is_some_and(|found| found.decl.kind == ClassKind::Trait);
            if is_trait {
                if !traits.contains(name) {
                    traits.push(name.clone());
                }
            } else {
                class = Some(name.clone());
            }
        }
    }
    let class = class.or_else(|| {
        index
            .class(crate::phpunit::TEST_CASE)
            .map(|found| found.decl.name.clone())
    })?;
    Some(TestCaseBinding { class, traits })
}

fn matches_folder(binding: &TestBinding, base: &Path, path: &Path) -> bool {
    deepest_match(binding, base, path).is_some()
}

/// How deep the folder of a binding is, when the file is in it.
fn deepest_match(binding: &TestBinding, base: &Path, path: &Path) -> Option<usize> {
    binding
        .folders
        .iter()
        .filter_map(|folder| {
            let folder = folder.trim_end_matches(['/', '*']);
            let full = if folder == "." || folder.is_empty() {
                base.to_path_buf()
            } else {
                base.join(folder)
            };
            path.starts_with(&full).then(|| full.components().count())
        })
        .max()
}

impl Analyzer<'_> {
    /// The type the properties set on `$this` in the `beforeEach` of a Pest file give `name`.
    pub(crate) fn pest_property(&self, name: &str) -> Option<Type> {
        let scope = self.pest.as_ref()?;
        let describing: Vec<u32> = scope
            .closure
            .ancestors()
            .filter(|ancestor| matches!(closure_owner(ancestor), Some((Dsl::Describe, _))))
            .map(|ancestor| crate::ast::start(&ancestor))
            .collect();
        let key = (describing, name.to_string());
        if let Some(found) = self.shared.pest_types.borrow().get(&key) {
            return (!found.is_unknown()).then(|| found.clone());
        }
        self.shared.pest_types.borrow_mut().insert(key.clone(), Type::Unknown);
        let assignments = self.pest_assignments(Some(name));
        let types: Vec<Type> = assignments
            .iter()
            .map(|value| {
                let env = self.env_around(value);
                self.type_of(value, &env)
            })
            .collect();
        let result = if types.is_empty() || types.iter().any(Type::is_unknown) {
            Type::Unknown
        } else {
            Type::union(types)
        };
        self.shared.pest_types.borrow_mut().insert(key, result.clone());
        (!result.is_unknown()).then_some(result)
    }

    /// The names `beforeEach` sets on `$this` for the test being read.
    pub(crate) fn pest_property_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for (name, _) in self.pest_assignments_named() {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names
    }

    /// The values assigned to `$this->name` (every name when `None`) in the hooks that run before
    /// the test: those of the file and of the `describe` blocks the test is in.
    fn pest_assignments(&self, name: Option<&str>) -> Vec<SyntaxNode> {
        self.pest_assignments_named()
            .into_iter()
            .filter(|(assigned, _)| name.is_none_or(|name| name == assigned))
            .map(|(_, value)| value)
            .collect()
    }

    fn pest_assignments_named(&self) -> Vec<(String, SyntaxNode)> {
        let Some(scope) = &self.pest else {
            return Vec::new();
        };
        let mut containers: Vec<SyntaxNode> = vec![self.root.clone()];
        for ancestor in scope.closure.ancestors() {
            if let Some((Dsl::Describe, _)) = closure_owner(&ancestor) {
                if let Some(body) = body_of(&ancestor) {
                    containers.push(body);
                }
            }
        }
        let mut out = Vec::new();
        for container in containers {
            for statement in statements_in(&container) {
                if statement.dsl != Dsl::BeforeEach {
                    continue;
                }
                let Some(closure) = statement.closure() else {
                    continue;
                };
                collect_assignments(&closure, &mut out);
            }
        }
        out
    }
}

/// The `$this->name = value` assignments of a hook's own body, as the name and the value.
fn collect_assignments(closure: &SyntaxNode, out: &mut Vec<(String, SyntaxNode)>) {
    for node in closure.descendants().filter(|node| node.kind() == ASSIGN_EXPR) {
        let owner = node
            .ancestors()
            .skip(1)
            .find(|ancestor| crate::ast::is_function_like(ancestor.kind()));
        if owner.as_ref() != Some(closure) {
            continue;
        }
        let operands: Vec<SyntaxNode> = node.children().collect();
        let [target, value] = operands.as_slice() else {
            continue;
        };
        let is_plain = crate::ast::tokens(&node)
            .find(|token| !token.kind().is_trivia())
            .is_some_and(|token| matches!(token.kind(), ASSIGN | COALESCE_ASSIGN));
        if target.kind() != PROPERTY_FETCH_EXPR || !is_plain {
            continue;
        }
        let mut parts = target.children();
        let (Some(object), Some(name)) = (parts.next(), parts.next()) else {
            continue;
        };
        if object.kind() == VARIABLE_EXPR && object.text() == "$this" && name.kind() == NAME {
            out.push((name.text().to_string(), value.clone()));
        }
    }
}
