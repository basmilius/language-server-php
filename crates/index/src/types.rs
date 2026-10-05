//! The type a declaration or an expression has, as far as the language server follows it. A type
//! read from source (native or PHPDoc) is stored with its class names already resolved.

use std::collections::HashMap;
use std::fmt::{self, Write};

use serde::{Deserialize, Serialize};

/// A fully qualified name without the leading backslash.
pub type Name = String;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShapeField {
    /// `None` for a positional entry of a list shape.
    pub key: Option<String>,
    pub optional: bool,
    pub ty: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallableParam {
    pub ty: Type,
    pub optional: bool,
    pub variadic: bool,
    pub by_ref: bool,
    pub name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallableType {
    /// `Closure(...)` as opposed to `callable(...)`.
    pub closure: bool,
    pub params: Vec<CallableParam>,
    pub ret: Option<Type>,
}

/// `($param is Foo ? A : B)` or `(T is Foo ? A : B)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conditional {
    /// The subject: a parameter name starting with `$`, or a template name.
    pub subject: String,
    pub is: Type,
    pub negated: bool,
    pub then: Type,
    pub otherwise: Type,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Type {
    /// Nothing is known: the inference gave up here.
    Unknown,
    Mixed,
    Void,
    Never,
    Null,
    Bool,
    True,
    False,
    Int,
    Float,
    String,
    Object,
    Resource,
    /// `int|string`.
    ArrayKey,
    /// `int|float|numeric-string`.
    Numeric,
    /// `int|float|string|bool`.
    Scalar,
    IntLiteral(i64),
    StringLiteral(String),
    /// `array<K, V>`; a bare `array` is `array<array-key, mixed>`.
    Array(Box<Type>, Box<Type>),
    /// `list<V>`: an array with keys 0, 1, 2 and so on.
    List(Box<Type>),
    Shape(Vec<ShapeField>),
    Iterable(Box<Type>, Box<Type>),
    Callable(Option<Box<CallableType>>),
    Class {
        name: Name,
        args: Vec<Type>,
    },
    /// The class a method is called on; `$this` is the same type.
    Static,
    /// `self` in a trait, where it is the class that uses the trait.
    SelfType,
    /// `parent` where the class has no parent in sight.
    Parent,
    /// A name from `@template`.
    Template(String),
    /// `class-string<T>`, or `class-string` without a bound.
    ClassString(Option<Box<Type>>),
    Union(Vec<Type>),
    Intersection(Vec<Type>),
    Conditional(Box<Conditional>),
}

impl Type {
    pub fn class(name: impl Into<Name>) -> Type {
        Type::Class {
            name: name.into(),
            args: Vec::new(),
        }
    }

    pub fn array_of(value: Type) -> Type {
        Type::Array(Box::new(Type::ArrayKey), Box::new(value))
    }

    pub fn plain_array() -> Type {
        Type::array_of(Type::Mixed)
    }

    /// A union with its members flattened and repeated ones dropped. A lone member is itself, and
    /// no member at all is `never`.
    pub fn union(types: impl IntoIterator<Item = Type>) -> Type {
        let mut flat: Vec<Type> = Vec::new();
        for ty in types {
            match ty {
                Type::Union(inner) => {
                    for member in inner {
                        if !flat.contains(&member) {
                            flat.push(member);
                        }
                    }
                }
                Type::Never => {}
                other => {
                    if !flat.contains(&other) {
                        flat.push(other);
                    }
                }
            }
        }
        if flat.iter().any(|ty| matches!(ty, Type::Mixed)) {
            return Type::Mixed;
        }
        // `A|(A&B)` is `A`.
        let plain: Vec<Type> = flat
            .iter()
            .filter(|ty| !matches!(ty, Type::Intersection(_)))
            .cloned()
            .collect();
        flat.retain(|ty| match ty {
            Type::Intersection(parts) => !parts.iter().any(|part| plain.contains(part)),
            _ => true,
        });
        let is_empty_array =
            |ty: &Type| matches!(ty, Type::Array(key, value) if **key == Type::Never && **value == Type::Never);
        if flat.iter().any(is_empty_array)
            && flat
                .iter()
                .any(|ty| matches!(ty, Type::Array(..) | Type::List(_) | Type::Shape(_)) && !is_empty_array(ty))
        {
            flat.retain(|ty| !is_empty_array(ty));
        }
        if flat.contains(&Type::True) && flat.contains(&Type::False) {
            flat.retain(|ty| !matches!(ty, Type::True | Type::False));
            flat.push(Type::Bool);
        }
        match flat.len() {
            0 => Type::Never,
            1 => flat.remove(0),
            _ => Type::Union(flat),
        }
    }

    /// This type with `null` added.
    pub fn nullable(self) -> Type {
        Type::union([self, Type::Null])
    }

    /// The members of a union, or the type itself.
    pub fn members(&self) -> &[Type] {
        match self {
            Type::Union(members) => members,
            _ => std::slice::from_ref(self),
        }
    }

    pub fn is_unknown(&self) -> bool {
        match self {
            Type::Unknown => true,
            Type::Union(members) => members.iter().all(Type::is_unknown),
            _ => false,
        }
    }

    pub fn contains_null(&self) -> bool {
        self.members().iter().any(|ty| matches!(ty, Type::Null))
    }

    /// The type without `null`, which is what `->` finds members on.
    pub fn without_null(&self) -> Type {
        Type::union(self.members().iter().filter(|ty| !matches!(ty, Type::Null)).cloned())
    }

    /// The members of a union that satisfy a predicate, as a type.
    pub fn filter(&self, keep: impl Fn(&Type) -> bool) -> Type {
        Type::union(self.members().iter().filter(|ty| keep(ty)).cloned())
    }

    /// Whether a template name is left in the type, which nothing bound to a type.
    pub fn has_template(&self) -> bool {
        match self {
            Type::Template(_) => true,
            Type::Array(key, value) | Type::Iterable(key, value) => key.has_template() || value.has_template(),
            Type::List(value) => value.has_template(),
            Type::Class { args, .. } => args.iter().any(Type::has_template),
            Type::ClassString(Some(inner)) => inner.has_template(),
            Type::Union(members) | Type::Intersection(members) => members.iter().any(Type::has_template),
            Type::Shape(fields) => fields.iter().any(|field| field.ty.has_template()),
            _ => false,
        }
    }

    /// Replaces template names and `static`/`self` where a map has an answer for them.
    pub fn substitute(
        &self,
        templates: &HashMap<String, Type>,
        static_type: Option<&Type>,
        self_name: Option<&str>,
    ) -> Type {
        let sub = |ty: &Type| ty.substitute(templates, static_type, self_name);
        match self {
            Type::Template(name) => templates.get(name).cloned().unwrap_or_else(|| self.clone()),
            Type::Static => static_type.cloned().unwrap_or(Type::Static),
            Type::SelfType => match self_name {
                Some(name) => Type::class(name),
                None => Type::SelfType,
            },
            Type::Array(key, value) => Type::Array(Box::new(sub(key)), Box::new(sub(value))),
            Type::List(value) => Type::List(Box::new(sub(value))),
            Type::Iterable(key, value) => Type::Iterable(Box::new(sub(key)), Box::new(sub(value))),
            Type::Shape(fields) => Type::Shape(
                fields
                    .iter()
                    .map(|field| ShapeField {
                        key: field.key.clone(),
                        optional: field.optional,
                        ty: sub(&field.ty),
                    })
                    .collect(),
            ),
            Type::Class { name, args } => Type::Class {
                name: name.clone(),
                args: args.iter().map(sub).collect(),
            },
            Type::ClassString(Some(inner)) => Type::ClassString(Some(Box::new(sub(inner)))),
            Type::Union(members) => Type::union(members.iter().map(sub)),
            Type::Intersection(members) => Type::Intersection(members.iter().map(sub).collect()),
            Type::Callable(Some(callable)) => Type::Callable(Some(Box::new(CallableType {
                closure: callable.closure,
                params: callable
                    .params
                    .iter()
                    .map(|param| CallableParam {
                        ty: sub(&param.ty),
                        ..param.clone()
                    })
                    .collect(),
                ret: callable.ret.as_ref().map(sub),
            }))),
            Type::Conditional(conditional) => Type::Conditional(Box::new(Conditional {
                subject: conditional.subject.clone(),
                is: sub(&conditional.is),
                negated: conditional.negated,
                then: sub(&conditional.then),
                otherwise: sub(&conditional.otherwise),
            })),
            other => other.clone(),
        }
    }

    /// Every class name the type mentions at its top level: the members of a union or an intersection.
    pub fn class_names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_class_names(&mut out);
        out
    }

    fn collect_class_names<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Type::Class { name, .. } => out.push(name),
            Type::Union(members) | Type::Intersection(members) => {
                for member in members {
                    member.collect_class_names(out);
                }
            }
            _ => {}
        }
    }

    /// The type as PHP source would write it. With `short` a class is its last name segment.
    pub fn display(&self, short: bool) -> String {
        self.display_with(&mut |name| {
            if short {
                short_name(name).to_string()
            } else {
                name.to_string()
            }
        })
    }

    /// The type as PHP source would write it, with every class written the way `class` says.
    pub fn display_with(&self, class: &mut dyn FnMut(&str) -> String) -> String {
        let mut out = String::new();
        self.write(&mut out, class);
        out
    }

    fn write(&self, out: &mut String, class: &mut dyn FnMut(&str) -> String) {
        match self {
            Type::Unknown | Type::Mixed => out.push_str("mixed"),
            Type::Void => out.push_str("void"),
            Type::Never => out.push_str("never"),
            Type::Null => out.push_str("null"),
            Type::Bool => out.push_str("bool"),
            Type::True => out.push_str("true"),
            Type::False => out.push_str("false"),
            Type::Int => out.push_str("int"),
            Type::Float => out.push_str("float"),
            Type::String => out.push_str("string"),
            Type::Object => out.push_str("object"),
            Type::Resource => out.push_str("resource"),
            Type::ArrayKey => out.push_str("array-key"),
            Type::Numeric => out.push_str("numeric"),
            Type::Scalar => out.push_str("scalar"),
            Type::IntLiteral(value) => {
                let _ = write!(out, "{value}");
            }
            Type::StringLiteral(value) => {
                let _ = write!(out, "'{value}'");
            }
            Type::Array(key, value) => {
                if **key == Type::ArrayKey && **value == Type::Mixed {
                    out.push_str("array");
                } else if **key == Type::ArrayKey {
                    out.push_str("array<");
                    value.write(out, class);
                    out.push('>');
                } else {
                    out.push_str("array<");
                    key.write(out, class);
                    out.push_str(", ");
                    value.write(out, class);
                    out.push('>');
                }
            }
            Type::List(value) => {
                out.push_str("list<");
                value.write(out, class);
                out.push('>');
            }
            Type::Shape(fields) => {
                out.push_str("array{");
                for (index, field) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    if let Some(key) = &field.key {
                        out.push_str(key);
                        if field.optional {
                            out.push('?');
                        }
                        out.push_str(": ");
                    }
                    field.ty.write(out, class);
                }
                out.push('}');
            }
            Type::Iterable(key, value) => {
                if **key == Type::Mixed && **value == Type::Mixed {
                    out.push_str("iterable");
                } else {
                    out.push_str("iterable<");
                    key.write(out, class);
                    out.push_str(", ");
                    value.write(out, class);
                    out.push('>');
                }
            }
            Type::Callable(None) => out.push_str("callable"),
            Type::Callable(Some(callable)) => {
                out.push_str(if callable.closure { "Closure(" } else { "callable(" });
                for (index, param) in callable.params.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    param.ty.write(out, class);
                    if param.variadic {
                        out.push_str("...");
                    }
                    if param.optional {
                        out.push('=');
                    }
                }
                out.push(')');
                if let Some(ret) = &callable.ret {
                    out.push_str(": ");
                    ret.write(out, class);
                }
            }
            Type::Class { name, args } => {
                out.push_str(&class(name));
                if !args.is_empty() {
                    out.push('<');
                    for (index, arg) in args.iter().enumerate() {
                        if index > 0 {
                            out.push_str(", ");
                        }
                        arg.write(out, class);
                    }
                    out.push('>');
                }
            }
            Type::Static => out.push_str("static"),
            Type::SelfType => out.push_str("self"),
            Type::Parent => out.push_str("parent"),
            Type::Template(name) => out.push_str(name),
            Type::ClassString(None) => out.push_str("class-string"),
            Type::ClassString(Some(inner)) => {
                out.push_str("class-string<");
                inner.write(out, class);
                out.push('>');
            }
            Type::Union(members) => {
                let others: Vec<&Type> = members.iter().filter(|ty| !matches!(ty, Type::Null)).collect();
                if others.len() == 1 && members.len() == 2 && !matches!(others[0], Type::Callable(_)) {
                    out.push('?');
                    others[0].write(out, class);
                    return;
                }
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        out.push('|');
                    }
                    let wrap = matches!(member, Type::Intersection(_) | Type::Conditional(_));
                    if wrap {
                        out.push('(');
                    }
                    member.write(out, class);
                    if wrap {
                        out.push(')');
                    }
                }
            }
            Type::Intersection(members) => {
                for (index, member) in members.iter().enumerate() {
                    if index > 0 {
                        out.push('&');
                    }
                    member.write(out, class);
                }
            }
            Type::Conditional(conditional) => {
                let _ = write!(out, "({} is ", conditional.subject);
                if conditional.negated {
                    out.push_str("not ");
                }
                conditional.is.write(out, class);
                out.push_str(" ? ");
                conditional.then.write(out, class);
                out.push_str(" : ");
                conditional.otherwise.write(out, class);
                out.push(')');
            }
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display(false))
    }
}

/// The part of a qualified name after its last backslash.
pub fn short_name(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

/// The namespace of a qualified name, without a trailing backslash.
pub fn namespace_of(name: &str) -> &str {
    name.rsplit_once('\\').map_or("", |(namespace, _)| namespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unions_flatten_and_collapse() {
        let ty = Type::union([Type::Int, Type::union([Type::String, Type::Int]), Type::Never]);
        assert_eq!(ty, Type::Union(vec![Type::Int, Type::String]));
        assert_eq!(Type::union([Type::True, Type::False]), Type::Bool);
        assert_eq!(Type::union([Type::Int, Type::Mixed]), Type::Mixed);
        assert_eq!(Type::union([]), Type::Never);
    }

    #[test]
    fn displays_as_php_source() {
        assert_eq!(Type::class("App\\User").nullable().display(true), "?User");
        assert_eq!(Type::class("App\\User").nullable().display(false), "?App\\User");
        assert_eq!(Type::plain_array().display(true), "array");
        assert_eq!(Type::List(Box::new(Type::Int)).display(true), "list<int>");
        assert_eq!(
            Type::Union(vec![Type::Int, Type::String, Type::Null]).display(true),
            "int|string|null"
        );
    }

    #[test]
    fn substitutes_templates_and_static() {
        let mut map = HashMap::new();
        map.insert("T".to_string(), Type::class("Foo"));
        let ty = Type::Array(Box::new(Type::Int), Box::new(Type::Template("T".into())));
        assert_eq!(ty.substitute(&map, None, None).display(true), "array<int, Foo>");
        assert_eq!(
            Type::Static.substitute(&map, Some(&Type::class("Bar")), None),
            Type::class("Bar")
        );
    }
}
