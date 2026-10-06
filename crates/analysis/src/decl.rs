//! Where a symbol is declared, in the files the index knows.

use std::path::PathBuf;

use php_index::{Callable, Index, Origin, Span};

use crate::refs::{Query, Symbol};
use crate::target::Callee;

/// A declaration of a symbol, with the file it is in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub path: PathBuf,
    pub origin: Origin,
    pub name_span: Span,
    /// The whole declaration, with its doc comment.
    pub span: Span,
}

/// The declarations of a symbol and of the members that are one thing with it.
pub fn declarations(index: &Index, query: &Query) -> Vec<Declaration> {
    let mut out = Vec::new();
    match &query.symbol {
        Symbol::Class(name) => {
            if let Some(class) = index.class(name) {
                out.push(Declaration {
                    path: class.file.path.clone(),
                    origin: class.file.origin,
                    name_span: class.decl.name_span,
                    span: class.decl.span,
                });
            }
        }
        Symbol::Function(name) => {
            if let Some(function) = index.function(name) {
                out.push(Declaration {
                    path: function.file.path.clone(),
                    origin: function.file.origin,
                    name_span: function.decl.name_span,
                    span: function.decl.span,
                });
            }
        }
        Symbol::Constant(name) => {
            if let Some(constant) = index.constant(name) {
                out.push(Declaration {
                    path: constant.file.path.clone(),
                    origin: constant.file.origin,
                    name_span: constant.decl.name_span,
                    span: constant.decl.span,
                });
            }
        }
        Symbol::Method { name, .. } => {
            for class in family_classes(index, query) {
                if let Some(method) = class.decl.method(name) {
                    out.push(Declaration {
                        path: class.file.path.clone(),
                        origin: class.file.origin,
                        name_span: method.name_span,
                        span: method.span,
                    });
                }
            }
        }
        Symbol::Property { name, .. } => {
            for class in family_classes(index, query) {
                if let Some(property) = class.decl.property(name) {
                    out.push(Declaration {
                        path: class.file.path.clone(),
                        origin: class.file.origin,
                        name_span: property.name_span,
                        span: property.span,
                    });
                }
            }
        }
        Symbol::ClassConst { name, .. } => {
            for class in family_classes(index, query) {
                if let Some(constant) = class.decl.constant(name) {
                    out.push(Declaration {
                        path: class.file.path.clone(),
                        origin: class.file.origin,
                        name_span: constant.name_span,
                        span: constant.span,
                    });
                }
            }
        }
        Symbol::Parameter { callee, name } => {
            if let Some((path, origin, callable)) = callable_of(index, callee) {
                if let Some(param) = callable.params.iter().find(|param| &param.name == name) {
                    out.push(Declaration {
                        path,
                        origin,
                        name_span: param.span,
                        span: param.span,
                    });
                }
            }
        }
        Symbol::Variable { .. } => {}
        Symbol::Dataset(name) => out.extend(crate::pest::dataset_declarations(index, name)),
        Symbol::Key { kind, name, scope } => {
            for definition in php_index::framework::keys::definitions(index, *kind, name, scope.as_deref()) {
                out.push(Declaration {
                    origin: index.file(&definition.path).map_or(Origin::Project, |file| file.origin),
                    path: definition.path,
                    name_span: definition.span,
                    span: definition.span,
                });
            }
        }
    }
    out
}

fn family_classes<'a>(index: &'a Index, query: &Query) -> Vec<php_index::Class<'a>> {
    let mut classes: Vec<php_index::Class<'a>> = query.family().iter().filter_map(|name| index.class(name)).collect();
    classes.sort_by(|left, right| left.decl.name.cmp(&right.decl.name));
    classes
}

fn callable_of(index: &Index, callee: &Callee) -> Option<(PathBuf, Origin, Callable)> {
    match callee {
        Callee::Function(name) => {
            let function = index.function(name)?;
            Some((
                function.file.path.clone(),
                function.file.origin,
                function.decl.callable.clone(),
            ))
        }
        Callee::Method { class, name } => {
            let class = index.class(class)?;
            let method = class.decl.method(name)?;
            Some((class.file.path.clone(), class.file.origin, method.callable.clone()))
        }
    }
}
