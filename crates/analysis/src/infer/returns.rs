//! Return types read from a body, for a function that declares none.

use std::rc::Rc;

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::calls::{DeclRef, ResolvedCallable};
use super::{Analyzer, Env};
use crate::ast::{child_of, is_function_like, node_at, start};

/// How many bodies deep one inference may read through the bodies it calls.
const MAX_BODIES: u32 = 3;

impl Analyzer<'_> {
    /// What a function or method with no declared return type returns, read from its body, or
    /// `Type::Unknown` when any return is not known.
    pub(super) fn inferred_return(&self, callee: &ResolvedCallable) -> Type {
        let Some(decl) = &callee.decl else {
            return Type::Unknown;
        };
        let key = (decl.path.clone(), decl.name_start);
        if let Some(found) = self.shared.returns.borrow().get(&key) {
            return found.clone();
        }
        let shared = &self.shared;
        if shared.depth.get() >= MAX_BODIES || !shared.in_progress.borrow_mut().insert(key.clone()) {
            return Type::Unknown;
        }
        shared.depth.set(shared.depth.get() + 1);
        let result = self.read_declaration(decl).map_or(Type::Unknown, |(root, function)| {
            let analyzer = Analyzer::with_shared(self.index, &root, decl.name_start, Rc::clone(shared));
            analyzer.infer_body_return(&function)
        });
        shared.depth.set(shared.depth.get() - 1);
        shared.in_progress.borrow_mut().remove(&key);
        if shared.depth.get() == 0 || !result.is_unknown() {
            shared.returns.borrow_mut().insert(key, result.clone());
        }
        result
    }

    /// The tree of a file on disk, read once for every analyzer working on the same file.
    pub(crate) fn parse_file(&self, path: &std::path::Path) -> Option<SyntaxNode> {
        let cached = self.shared.trees.borrow().get(path).cloned();
        match cached {
            Some(root) => root,
            None => {
                let bytes = std::fs::read(path).ok();
                let root = bytes.map(|bytes| parse(&String::from_utf8_lossy(&bytes)).syntax());
                self.shared.trees.borrow_mut().insert(path.to_path_buf(), root.clone());
                root
            }
        }
    }

    /// The declaration node at a place: in the tree this analyzer works on when the name is there,
    /// else in the file on disk.
    pub(crate) fn read_declaration(&self, decl: &DeclRef) -> Option<(SyntaxNode, SyntaxNode)> {
        if let Some(function) = declaration_at(&self.root, decl.name_start) {
            return Some((self.root.clone(), function));
        }
        let root = self.parse_file(&decl.path)?;
        let function = declaration_at(&root, decl.name_start)?;
        Some((root, function))
    }

    /// The type a function-like node returns from what its body returns and yields. A closure keeps
    /// its answer per node, since a call asks for it once for each argument it binds.
    pub fn infer_body_return(&self, function: &SyntaxNode) -> Type {
        if matches!(function.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR) {
            let key = start(function);
            if let Some(found) = self.closure_returns.borrow().get(&key) {
                return found.clone();
            }
            self.closure_returns.borrow_mut().insert(key, Type::Unknown);
            let result = self.read_body_return(function);
            self.closure_returns.borrow_mut().insert(key, result.clone());
            return result;
        }
        self.read_body_return(function)
    }

    fn read_body_return(&self, function: &SyntaxNode) -> Type {
        let (returns, yields) = collect_exits(function);
        let mut returned: Vec<Type> = Vec::new();
        for statement in &returns {
            match statement.children().next() {
                Some(value) => {
                    let env = self.env_around(&value);
                    returned.push(self.type_of(&value, &env));
                }
                None => returned.push(Type::Null),
            }
        }
        if function.kind() == ARROW_FUNCTION_EXPR {
            if let Some(body) = function.children().last().filter(|body| {
                !matches!(
                    body.kind(),
                    PARAMETER_LIST | NAMED_TYPE | NULLABLE_TYPE | UNION_TYPE | INTERSECTION_TYPE
                )
            }) {
                let env = self.env_around(&body);
                returned.push(self.type_of(&body, &env));
            }
        }
        if !yields.is_empty() {
            return self.generator_type(&yields, &returned);
        }
        if returned.is_empty() || returned.iter().any(Type::is_unknown) {
            return Type::Unknown;
        }
        Type::union(returned)
    }

    fn generator_type(&self, yields: &[SyntaxNode], returned: &[Type]) -> Type {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for expression in yields {
            let env: Rc<Env> = self.env_around(expression);
            let operands: Vec<SyntaxNode> = expression.children().collect();
            if expression.kind() == YIELD_FROM_EXPR {
                let source = operands
                    .first()
                    .map_or(Type::Unknown, |operand| self.type_of(operand, &env));
                let (key, value) = self.iterable_types(&source);
                keys.push(key);
                values.push(value);
                continue;
            }
            match operands.as_slice() {
                [key, value] => {
                    keys.push(self.type_of(key, &env));
                    values.push(self.type_of(value, &env));
                }
                [value] => {
                    keys.push(Type::Int);
                    values.push(self.type_of(value, &env));
                }
                _ => {
                    keys.push(Type::Int);
                    values.push(Type::Null);
                }
            }
        }
        let known = |types: Vec<Type>| {
            if types.iter().any(Type::is_unknown) {
                Type::Mixed
            } else {
                Type::union(types)
            }
        };
        let result = if returned.is_empty() {
            Type::Null
        } else {
            known(returned.to_vec())
        };
        Type::Class {
            name: "Generator".to_string(),
            args: vec![known(keys), known(values), Type::Mixed, result],
        }
    }
}

/// The method or function declaration whose name starts at an offset.
fn declaration_at(root: &SyntaxNode, name_start: u32) -> Option<SyntaxNode> {
    let node = node_at(root, name_start + 1);
    node.ancestors()
        .find(|ancestor| matches!(ancestor.kind(), METHOD_DECLARATION | FUNCTION_DECLARATION))
        .filter(|declaration| child_of(declaration, NAME).is_some_and(|name| start(&name) == name_start))
}

/// The `return` statements and the `yield` expressions of a function's own body, not those of the
/// closures and classes inside it.
fn collect_exits(function: &SyntaxNode) -> (Vec<SyntaxNode>, Vec<SyntaxNode>) {
    let mut returns = Vec::new();
    let mut yields = Vec::new();
    for node in function.descendants() {
        if !matches!(node.kind(), RETURN_STATEMENT | YIELD_EXPR | YIELD_FROM_EXPR) {
            continue;
        }
        let owner = node
            .ancestors()
            .skip(1)
            .find(|ancestor| is_function_like(ancestor.kind()) || crate::ast::is_class_like(ancestor.kind()));
        if owner.as_ref() != Some(function) {
            continue;
        }
        if node.kind() == RETURN_STATEMENT {
            returns.push(node);
        } else {
            yields.push(node);
        }
    }
    (returns, yields)
}
