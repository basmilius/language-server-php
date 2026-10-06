//! The variables of a Twig template and the types of its expressions. A template is walked once, in
//! order, with the variables in force at each expression: what the template is given, what `for`,
//! `set`, `macro` and `with` add. `post.title` is looked up the way Twig's `getAttribute` does: a
//! key of an array, a public property, then the method `title()`, `getTitle()`, `isTitle()` or
//! `hasTitle()`.

use php_index::framework::twig::{TwigExtensions, TwigKind};
use php_index::types::ShapeField;
use php_index::{Index, Type, Visibility};
use php_syntax::SyntaxNode;

use super::parse::{Expr, HashKey};
use super::{Item, TagBody, Template};
use crate::infer::{Analyzer, Env};

/// A member an attribute names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Member {
    Property { class: String, name: String },
    Method { class: String, name: String },
}

/// The variables of `loop` inside a `for`.
fn loop_type() -> Type {
    let field = |key: &str, ty: Type| ShapeField {
        key: Some(key.to_string()),
        ty,
        optional: false,
    };
    Type::Shape(vec![
        field("index", Type::Int),
        field("index0", Type::Int),
        field("revindex", Type::Int),
        field("revindex0", Type::Int),
        field("first", Type::Bool),
        field("last", Type::Bool),
        field("length", Type::Int),
        field("parent", Type::plain_array()),
    ])
}

/// The variables every template of a Symfony project has.
pub fn globals(index: &Index) -> Vec<(String, Type)> {
    let mut out = Vec::new();
    if index.class("Symfony\\Bridge\\Twig\\AppVariable").is_some() {
        out.push(("app".to_string(), Type::class("Symfony\\Bridge\\Twig\\AppVariable")));
    }
    out
}

pub struct Typer<'a> {
    pub index: &'a Index,
    extensions: std::sync::Arc<TwigExtensions>,
    root: SyntaxNode,
}

impl<'a> Typer<'a> {
    pub fn new(index: &'a Index) -> Typer<'a> {
        Typer {
            index,
            extensions: index.section::<TwigExtensions>(),
            root: php_syntax::parse("<?php ").syntax(),
        }
    }

    fn analyzer(&self) -> Analyzer<'a> {
        Analyzer::new(self.index, &self.root, 0)
    }

    pub fn type_of(&self, expr: &Expr, env: &Env) -> Type {
        match expr {
            Expr::Name { name, .. } => env.get(name).cloned().unwrap_or(Type::Unknown),
            Expr::Str { value, .. } => Type::StringLiteral(value.clone()),
            Expr::Number => Type::Int,
            Expr::Literal(text) => match text.as_str() {
                "true" | "false" => Type::Bool,
                _ => Type::Null,
            },
            Expr::Array(items) => {
                let members: Vec<Type> = items.iter().map(|item| widen(self.type_of(item, env))).collect();
                if members.is_empty() {
                    Type::plain_array()
                } else {
                    Type::List(Box::new(Type::union(members)))
                }
            }
            Expr::Hash(pairs) => Type::Shape(
                pairs
                    .iter()
                    .map(|(key, value)| ShapeField {
                        key: match key {
                            HashKey::Name(name, ..) | HashKey::Str(name, ..) => Some(name.clone()),
                            HashKey::Expr(_) => None,
                        },
                        ty: widen(self.type_of(value, env)),
                        optional: false,
                    })
                    .collect(),
            ),
            Expr::Attribute { object, name, args, .. } => {
                let object = self.type_of(object, env);
                self.attribute(&object, name, args.is_some()).0
            }
            Expr::Index { object, index } => {
                let object = self.type_of(object, env);
                let key = match index.as_ref() {
                    Expr::Str { value, .. } => Some(value.as_str()),
                    _ => None,
                };
                self.analyzer().element_of(&object, key)
            }
            Expr::Call { name, .. } => self.callable_return(TwigKind::Function, name),
            Expr::Filter {
                subject, name, args, ..
            } => self.filter_type(subject, name, args, env),
            Expr::Test { .. } => Type::Bool,
            Expr::Unary { op, operand } => match op.as_str() {
                "not" => Type::Bool,
                _ => self.type_of(operand, env),
            },
            Expr::Binary { op, left, right } => match op.as_str() {
                "~" => Type::String,
                "+" | "-" | "*" | "/" | "//" | "%" | "**" => Type::union([Type::Int, Type::Float]),
                ".." => Type::List(Box::new(Type::union([Type::Int, Type::String]))),
                "??" => Type::union([self.type_of(left, env).without_null(), self.type_of(right, env)]),
                _ => Type::Bool,
            },
            Expr::Conditional {
                condition,
                then,
                otherwise,
            } => {
                let then = then
                    .as_deref()
                    .map_or_else(|| self.type_of(condition, env), |then| self.type_of(then, env));
                let otherwise = otherwise
                    .as_deref()
                    .map_or(Type::Null, |otherwise| self.type_of(otherwise, env));
                Type::union([then, otherwise])
            }
            Expr::Arrow { .. } => Type::class("Closure"),
            Expr::Missing(_) => Type::Unknown,
        }
    }

    /// What a function or filter returns, by the PHP behind it.
    fn callable_return(&self, kind: TwigKind, name: &str) -> Type {
        let Some(callable) = self.extensions.find(kind, name) else {
            return Type::Unknown;
        };
        let level = self.index.level;
        let declared = match (&callable.class, &callable.target) {
            (Some(class), Some(method)) => {
                self.index
                    .find_method(&Type::class(class.clone()), method)
                    .and_then(|found| {
                        found
                            .member
                            .callable
                            .effective_return(level)
                            .map(|ty| found.resolve(ty))
                    })
            }
            (None, Some(function)) => self
                .index
                .function(function)
                .and_then(|found| found.decl.callable.effective_return(level).cloned()),
            _ => None,
        };
        declared.unwrap_or(Type::Unknown)
    }

    /// A filter that hands back what it is given, or an element of it, keeps its type; the others
    /// return what their PHP says.
    fn filter_type(&self, subject: &Expr, name: &str, args: &[super::parse::Arg], env: &Env) -> Type {
        let analyzer = self.analyzer();
        match name {
            "first" | "last" => analyzer.element_of(&self.type_of(subject, env), None),
            "sort" | "reverse" | "slice" | "filter" | "merge" | "shuffle" => self.type_of(subject, env),
            "default" => {
                let fallback = args.first().map_or(Type::Unknown, |arg| self.type_of(&arg.value, env));
                Type::union([self.type_of(subject, env).without_null(), fallback])
            }
            "raw" | "escape" | "e" => self.type_of(subject, env),
            "keys" => Type::List(Box::new(Type::ArrayKey)),
            _ => match self.callable_return(TwigKind::Filter, name) {
                Type::Unknown | Type::Mixed => Type::Unknown,
                ty => ty,
            },
        }
    }

    /// The type of `object.name` and the member it reads: a key of an array shape, a property, or
    /// the method Twig calls for it.
    pub fn attribute(&self, object: &Type, name: &str, called: bool) -> (Type, Option<Member>) {
        let mut types = Vec::new();
        let mut member = None;
        for each in object.members() {
            match each {
                Type::Shape(fields) => {
                    if let Some(field) = fields.iter().find(|field| field.key.as_deref() == Some(name)) {
                        types.push(field.ty.clone());
                    }
                }
                Type::Array(_, value) | Type::List(value) => types.push((**value).clone()),
                Type::Class { .. } => {
                    if let Some((ty, found)) = self.class_attribute(each, name, called) {
                        types.push(ty);
                        member.get_or_insert(found);
                    } else if let Some(ty) = self.offset_get(each) {
                        types.push(ty);
                    }
                }
                _ => {}
            }
        }
        if types.is_empty() {
            (Type::Unknown, member)
        } else {
            (Type::union(types), member)
        }
    }

    /// What `object.name` is on a class that is an `ArrayAccess` and has no member of that name:
    /// what its `offsetGet()` returns, as a form view's children are.
    fn offset_get(&self, class: &Type) -> Option<Type> {
        let Type::Class { name, .. } = class else {
            return None;
        };
        if !self.index.is_subclass_of(name, "ArrayAccess") {
            return None;
        }
        let method = self.index.find_method(class, "offsetGet")?;
        let ty = method.member.callable.effective_return(self.index.level)?;
        Some(self.analyzer().bind_static(&method.resolve(ty), class)).filter(|ty| !matches!(ty, Type::Mixed))
    }

    fn class_attribute(&self, class: &Type, name: &str, called: bool) -> Option<(Type, Member)> {
        let level = self.index.level;
        if !called {
            if let Some(property) = self
                .index
                .find_property(class, name)
                .filter(|found| found.member.visibility == Visibility::Public && !found.member.is_static)
            {
                let ty = property
                    .member
                    .effective_type(level)
                    .map_or(Type::Unknown, |ty| property.resolve(ty));
                return Some((
                    self.analyzer().bind_static(&ty, class),
                    Member::Property {
                        class: property.class.decl.name.clone(),
                        name: property.member.name.clone(),
                    },
                ));
            }
        }
        let mut upper = name.to_string();
        if let Some(first) = upper.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        for candidate in [
            name.to_string(),
            format!("get{upper}"),
            format!("is{upper}"),
            format!("has{upper}"),
        ] {
            let Some(method) = self
                .index
                .find_method(class, &candidate)
                .filter(|found| found.member.visibility == Visibility::Public)
            else {
                continue;
            };
            let ty = method
                .member
                .callable
                .effective_return(level)
                .map_or(Type::Unknown, |ty| method.resolve(ty));
            return Some((
                self.analyzer().bind_static(&ty, class),
                Member::Method {
                    class: method.class.decl.name.clone(),
                    name: method.member.name.clone(),
                },
            ));
        }
        None
    }

    /// What can follow `object.`: keys, public properties, and public methods by the name Twig
    /// reads them under (`title` for `getTitle()`).
    pub fn attributes_of(&self, object: &Type) -> Vec<(String, Option<String>)> {
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        let mut add = |name: String, detail: Option<String>| {
            if !out.iter().any(|(known, _)| *known == name) {
                out.push((name, detail));
            }
        };
        for each in object.members() {
            match each {
                Type::Shape(fields) => {
                    for field in fields {
                        if let Some(key) = &field.key {
                            add(key.clone(), Some(field.ty.display(true)));
                        }
                    }
                }
                Type::Class { .. } => {
                    for property in self.index.properties(each) {
                        if property.member.visibility == Visibility::Public && !property.member.is_static {
                            add(
                                property.member.name.clone(),
                                property.member.ty.as_ref().map(|ty| ty.display(true)),
                            );
                        }
                    }
                    for method in self.index.methods(each) {
                        let declared = &method.member;
                        if declared.visibility != Visibility::Public
                            || declared.is_static
                            || declared.name.starts_with("__")
                        {
                            continue;
                        }
                        let detail = Some(format!("{}()", declared.name));
                        add(twig_name(&declared.name), detail);
                    }
                }
                _ => {}
            }
        }
        out
    }
}

/// `getTitle` is `title` in Twig, `isPublished` is `published`.
pub fn twig_name(method: &str) -> String {
    for prefix in ["get", "is", "has"] {
        if let Some(rest) = method.strip_prefix(prefix) {
            if rest.starts_with(|first: char| first.is_ascii_uppercase()) {
                let mut name = rest.to_string();
                name[..1].make_ascii_lowercase();
                return name;
            }
        }
    }
    method.to_string()
}

/// A literal's own type for a variable that holds it: `'a'` is a string.
fn widen(ty: Type) -> Type {
    match ty {
        Type::StringLiteral(_) => Type::String,
        Type::IntLiteral(_) => Type::Int,
        other => other,
    }
}

/// Walks the template in order and calls `visit` with each expression and the variables in force
/// where it stands. `until` stops the walk at the item that holds an offset.
pub fn walk(
    typer: &Typer<'_>,
    template: &Template,
    given: &[(String, Type)],
    until: Option<u32>,
    visit: &mut dyn FnMut(&Expr, &Env),
) {
    let mut base = Env::default();
    for (name, ty) in globals(typer.index).into_iter().chain(given.iter().cloned()) {
        base.set(name, ty);
    }
    let globals_only = {
        let mut env = Env::default();
        for (name, ty) in globals(typer.index) {
            env.set(name, ty);
        }
        env
    };
    let mut stack: Vec<Env> = vec![base];
    for item in &template.items {
        let start = match item {
            Item::Output { start, .. } => *start,
            Item::Tag(tag) => tag.start,
        };
        if until.is_some_and(|until| start > until) {
            return;
        }
        let env = stack.last().cloned().unwrap_or_default();
        match item {
            Item::Output { expr, .. } => visit(expr, &env),
            Item::Tag(tag) => match (&tag.body, tag.name.as_str()) {
                (
                    TagBody::For {
                        targets,
                        iterable,
                        condition,
                    },
                    _,
                ) => {
                    visit(iterable, &env);
                    let (key, value) = typer.analyzer().iterable_types(&typer.type_of(iterable, &env));
                    let mut inner = env.clone();
                    match targets.as_slice() {
                        [single] => inner.set(single.0.clone(), value),
                        [first, second] => {
                            inner.set(first.0.clone(), key);
                            inner.set(second.0.clone(), value);
                        }
                        _ => {}
                    }
                    inner.set("loop", loop_type());
                    if let Some(condition) = condition {
                        visit(condition, &inner);
                    }
                    stack.push(inner);
                }
                (TagBody::Set { targets, values }, _) => {
                    for value in values {
                        visit(value, &env);
                    }
                    let types: Vec<Type> = values.iter().map(|value| widen(typer.type_of(value, &env))).collect();
                    let top = stack.last_mut().expect("a scope");
                    for (at, target) in targets.iter().enumerate() {
                        let ty = if values.is_empty() {
                            Type::String
                        } else {
                            types.get(at).cloned().unwrap_or(Type::Unknown)
                        };
                        top.set(target.0.clone(), ty);
                    }
                }
                (TagBody::Macro { params, .. }, _) => {
                    let mut inner = globals_only.clone();
                    for (param, default) in params {
                        let ty = default
                            .as_ref()
                            .map_or(Type::Unknown, |default| widen(typer.type_of(default, &env)));
                        inner.set(param.0.clone(), ty);
                    }
                    stack.push(inner);
                }
                (TagBody::Other(exprs), "with") => {
                    for expr in exprs {
                        visit(expr, &env);
                    }
                    let only = exprs
                        .iter()
                        .any(|expr| matches!(expr, Expr::Name { name, .. } if name == "only"));
                    let mut inner = if only { globals_only.clone() } else { env.clone() };
                    if let Some(hash) = exprs.first() {
                        for (key, value) in super::hash_keys(hash) {
                            inner.set(key, widen(typer.type_of(value, &env)));
                        }
                    }
                    stack.push(inner);
                }
                (_, "endfor" | "endmacro" | "endwith") => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                }
                (body, _) => {
                    for expr in super::tag_exprs(body) {
                        visit(expr, &env);
                    }
                }
            },
        }
    }
}
