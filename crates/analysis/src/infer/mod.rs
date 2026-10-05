//! A type layer good enough to complete members: what a variable or an expression is at a point of a
//! file. Declared types, PHPDoc types, assignments, `new`, calls with template arguments bound from
//! their arguments, `instanceof` and null checks, `foreach` and destructuring are followed. What is
//! not followed (references, loops reaching a fixed point, dynamic names) comes out as
//! `Type::Unknown`.

mod calls;
mod expr;
mod flow;
mod returns;
mod unify;

#[cfg(test)]
mod assert_tests;

#[cfg(test)]
mod closure_tests;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use php_index::extract::resolver_at;
use php_index::{ClassKind, Index, Name, NameResolver, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use crate::ast::{self, child_of, node_at, text_of};

pub use calls::{Arg, DeclRef, ResolvedCallable, arguments, is_first_class_callable};
pub use expr::literal_string;
pub use unify::template_names;

/// The variables in scope and their types, without the `$`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Env {
    pub vars: BTreeMap<String, Type>,
}

impl Env {
    pub fn get(&self, name: &str) -> Option<&Type> {
        self.vars.get(name)
    }

    pub fn set(&mut self, name: impl Into<String>, ty: Type) {
        self.vars.insert(name.into(), ty);
    }
}

/// The class around a point of a file.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassContext {
    pub name: Name,
    pub parent: Option<Name>,
    pub kind: ClassKind,
    pub anonymous: bool,
}

/// What analyzers working on one file share: bodies read from other files and the return types
/// worked out from them.
#[derive(Default)]
pub struct SharedCache {
    returns: RefCell<HashMap<(PathBuf, u32), Type>>,
    trees: RefCell<HashMap<PathBuf, Option<SyntaxNode>>>,
    in_progress: RefCell<HashSet<(PathBuf, u32)>>,
    depth: Cell<u32>,
    /// The test case bound to the file being read, worked out once.
    pub(crate) pest_case: RefCell<Option<Option<crate::pest::case::TestCaseBinding>>>,
    /// What `beforeEach` gives a property, by the describe blocks around the test and the name.
    pub(crate) pest_types: RefCell<HashMap<(Vec<u32>, String), Type>>,
    /// The expectations the project adds, for the tree they were read with.
    pub(crate) expectations: RefCell<Option<(SyntaxNode, Rc<Vec<crate::pest::CustomExpectation>>)>>,
}

pub struct Analyzer<'a> {
    pub index: &'a Index,
    pub root: SyntaxNode,
    /// The namespace and imports at the offset the analyzer was made for.
    pub resolver: NameResolver,
    pub class: Option<ClassContext>,
    depth: Cell<u32>,
    /// The variables at the start of a statement, by the offset of that start.
    envs: RefCell<HashMap<u32, Rc<Env>>>,
    /// The parameter types a call gives a closure, by the start of the closure.
    hints: RefCell<HashMap<u32, Vec<Type>>>,
    /// What a closure without a declared return type returns, by the start of the closure.
    closure_returns: RefCell<HashMap<u32, Type>>,
    pub(crate) shared: Rc<SharedCache>,
    /// Set inside the closure of a Pest test or hook, where `$this` is a test case.
    pub(crate) pest: Option<crate::pest::case::PestScope>,
}

const MAX_DEPTH: u32 = 48;

impl<'a> Analyzer<'a> {
    pub fn new(index: &'a Index, root: &SyntaxNode, offset: u32) -> Analyzer<'a> {
        Analyzer::with_shared(index, root, offset, Rc::default())
    }

    pub fn with_shared(index: &'a Index, root: &SyntaxNode, offset: u32, shared: Rc<SharedCache>) -> Analyzer<'a> {
        let resolver = resolver_at(root, offset);
        let path = crate::document::current();
        let pest = crate::pest::case::scope_at(index, root, offset, path.as_deref(), &shared);
        let class = class_context(root, offset, &resolver)
            .or_else(|| pest.as_ref().map(crate::pest::case::PestScope::class_context));
        Analyzer {
            index,
            root: root.clone(),
            resolver,
            class,
            depth: Cell::new(0),
            envs: RefCell::new(HashMap::new()),
            hints: RefCell::new(HashMap::new()),
            closure_returns: RefCell::new(HashMap::new()),
            shared,
            pest,
        }
    }

    /// The variables in scope where the statement around a node begins. Everything in one statement
    /// shares the answer, so a pass over a whole file follows each function body once per statement.
    pub fn env_around(&self, node: &SyntaxNode) -> Rc<Env> {
        let anchor = ast::statement_anchor(node);
        let cached = self.envs.borrow().get(&anchor).cloned();
        let base = match cached {
            Some(env) => env,
            None => {
                let env = Rc::new(self.env_at(anchor));
                self.envs.borrow_mut().insert(anchor, env.clone());
                env
            }
        };
        let conditions = short_circuit_conditions(node);
        if conditions.is_empty() {
            return base;
        }
        let mut env = (*base).clone();
        for (condition, truth) in conditions.into_iter().rev() {
            self.narrow(&condition, truth, &mut env);
        }
        Rc::new(env)
    }

    pub fn level(&self) -> php_syntax::PhpVersion {
        self.index.level
    }

    /// Runs a step of inference with a guard against runaway recursion.
    fn guarded<T>(&self, fallback: T, step: impl FnOnce() -> T) -> T {
        if self.depth.get() >= MAX_DEPTH {
            return fallback;
        }
        self.depth.set(self.depth.get() + 1);
        let result = step();
        self.depth.set(self.depth.get() - 1);
        result
    }

    /// The class a written name stands for in a type or `new`: `self`, `static` and `parent` follow
    /// the class around the offset.
    pub fn class_type(&self, raw: &str) -> Type {
        match raw.to_ascii_lowercase().as_str() {
            "self" => self
                .class
                .as_ref()
                .map_or(Type::Unknown, |class| Type::class(class.name.clone())),
            "static" => Type::Static,
            "parent" => self
                .class
                .as_ref()
                .and_then(|class| class.parent.clone())
                .map_or(Type::Unknown, Type::class),
            _ => Type::class(self.resolver.resolve_class(raw)),
        }
    }

    /// The type `$this` has: the enclosing class.
    pub fn this_type(&self) -> Type {
        if let Some(scope) = &self.pest {
            return scope.this_type();
        }
        match &self.class {
            Some(class) if class.anonymous => class.parent.clone().map_or(Type::Unknown, Type::class),
            Some(class) => Type::class(class.name.clone()),
            None => Type::Unknown,
        }
    }

    /// Binds `static` in a type to the receiver the member was found on.
    pub fn bind_static(&self, ty: &Type, receiver: &Type) -> Type {
        ty.substitute(&Default::default(), Some(receiver), None)
    }

    /// The function-like node around an offset, or the file.
    pub fn scope_at(&self, offset: u32) -> SyntaxNode {
        let node = node_at(&self.root, offset);
        for ancestor in node.ancestors() {
            if ast::is_function_like(ancestor.kind()) {
                return ancestor;
            }
        }
        self.root.clone()
    }
}

fn class_context(root: &SyntaxNode, offset: u32, resolver: &NameResolver) -> Option<ClassContext> {
    let node = node_at(root, offset);
    for ancestor in node.ancestors() {
        let kind = match ancestor.kind() {
            CLASS_DECLARATION => ClassKind::Class,
            INTERFACE_DECLARATION => ClassKind::Interface,
            TRAIT_DECLARATION => ClassKind::Trait,
            ENUM_DECLARATION => ClassKind::Enum,
            ANONYMOUS_CLASS => ClassKind::Class,
            _ => continue,
        };
        let anonymous = ancestor.kind() == ANONYMOUS_CLASS;
        let parent = child_of(&ancestor, EXTENDS_CLAUSE)
            .and_then(|clause| clause.descendants().find(|node| node.kind() == NAME))
            .map(|name| resolver.resolve_class(&text_of(&name)))
            .or_else(|| {
                anonymous
                    .then(|| {
                        child_of(&ancestor, IMPLEMENTS_CLAUSE)
                            .and_then(|clause| clause.descendants().find(|node| node.kind() == NAME))
                            .map(|name| resolver.resolve_class(&text_of(&name)))
                    })
                    .flatten()
            });
        let name = if anonymous {
            "class@anonymous".to_string()
        } else {
            child_of(&ancestor, NAME).map(|name| resolver.qualify(&text_of(&name)))?
        };
        return Some(ClassContext {
            name,
            parent,
            kind,
            anonymous,
        });
    }
    None
}

#[cfg(test)]
mod tests;

/// The conditions that hold where a node is evaluated because of the operators around it: the right
/// side of `&&` runs once the left was true, that of `||` once it was false, and the branches of a
/// ternary follow its condition. Innermost first, up to the statement or function the node is in.
fn short_circuit_conditions(node: &SyntaxNode) -> Vec<(SyntaxNode, bool)> {
    let mut out = Vec::new();
    let mut child = node.clone();
    while let Some(parent) = child.parent() {
        match parent.kind() {
            BINARY_EXPR => {
                let operands: Vec<SyntaxNode> = parent.children().collect();
                if operands.len() == 2 && operands[1] == child {
                    let operator = ast::tokens(&parent)
                        .find(|token| !token.kind().is_trivia())
                        .map(|token| token.kind());
                    match operator {
                        Some(AND_AND | AND_KW) => out.push((operands[0].clone(), true)),
                        Some(OR_OR | OR_KW) => out.push((operands[0].clone(), false)),
                        _ => {}
                    }
                }
            }
            TERNARY_EXPR => {
                let operands: Vec<SyntaxNode> = parent.children().collect();
                match operands.as_slice() {
                    [condition, then, _] if *then == child => out.push((condition.clone(), true)),
                    [condition, .., otherwise] if *otherwise == child && operands.len() >= 2 => {
                        out.push((condition.clone(), false));
                    }
                    _ => {}
                }
            }
            ARROW_FUNCTION_EXPR | CLOSURE_EXPR | EXPR_STATEMENT | ECHO_STATEMENT | RETURN_STATEMENT | BLOCK
            | STATEMENT_LIST | SOURCE_FILE => break,
            _ => {}
        }
        child = parent;
    }
    out
}
