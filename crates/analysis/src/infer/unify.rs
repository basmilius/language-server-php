//! Binding `@template` names from the arguments of a call, and the checks that conditional return
//! types ask of a type.

use std::collections::HashMap;

use php_index::Type;

use super::Analyzer;

impl Analyzer<'_> {
    /// Binds the templates a parameter type mentions to what an argument of type `arg` gives them.
    /// A template that already has an answer keeps it.
    pub fn bind_templates(&self, param: &Type, arg: &Type, map: &mut HashMap<String, Type>) {
        match param {
            Type::Template(name) => {
                if !map.contains_key(name) && !arg.is_unknown() {
                    map.insert(name.clone(), widen(arg));
                }
            }
            Type::Array(param_key, param_value) | Type::Iterable(param_key, param_value) => {
                let (key, value) = self.iterable_types(arg);
                if !key.is_unknown() {
                    self.bind_templates(param_key, &key, map);
                }
                if !value.is_unknown() {
                    self.bind_templates(param_value, &value, map);
                }
            }
            Type::List(param_value) => {
                let (_, value) = self.iterable_types(arg);
                if !value.is_unknown() {
                    self.bind_templates(param_value, &value, map);
                }
            }
            Type::ClassString(Some(inner)) => {
                for member in arg.members() {
                    if let Type::ClassString(Some(bound)) = member {
                        self.bind_templates(inner, bound, map);
                    }
                }
            }
            Type::Class { name, args } if !args.is_empty() => {
                for member in arg.members() {
                    if let Some(Type::Class { args: arg_args, .. }) = self.as_ancestor(member, name) {
                        for (param_arg, actual) in args.iter().zip(&arg_args) {
                            self.bind_templates(param_arg, actual, map);
                        }
                    }
                }
            }
            Type::Union(members) => {
                let non_null = arg.without_null();
                for member in members {
                    if !matches!(member, Type::Null) {
                        self.bind_templates(member, &non_null, map);
                    }
                }
            }
            Type::Callable(Some(signature)) => {
                for member in arg.members() {
                    if let Type::Callable(Some(actual)) = member {
                        for (param_param, actual_param) in signature.params.iter().zip(&actual.params) {
                            if !matches!(actual_param.ty, Type::Mixed) {
                                self.bind_templates(&param_param.ty, &actual_param.ty, map);
                            }
                        }
                        if let (Some(expected), Some(actual)) = (&signature.ret, &actual.ret) {
                            self.bind_templates(expected, actual, map);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// A class type seen as one of its ancestors, with the arguments that ancestor was reached with.
    pub fn as_ancestor(&self, ty: &Type, ancestor_name: &str) -> Option<Type> {
        if !matches!(ty, Type::Class { .. }) {
            return None;
        }
        for ancestor in self.index.ancestors(ty) {
            if !ancestor.class.decl.name.eq_ignore_ascii_case(ancestor_name) {
                continue;
            }
            let args = ancestor
                .class
                .decl
                .doc
                .iter()
                .flat_map(|doc| doc.templates.iter())
                .map(|template| ancestor.subst.get(&template.name).cloned().unwrap_or(Type::Unknown))
                .collect();
            return Some(Type::Class {
                name: ancestor.class.decl.name.clone(),
                args,
            });
        }
        None
    }

    /// Whether a type is, is not or may be an instance of an expected type.
    pub fn satisfies(&self, ty: &Type, expected: &Type) -> Option<bool> {
        if matches!(expected, Type::Mixed) {
            return Some(true);
        }
        let mut any_yes = false;
        let mut any_no = false;
        let mut any_unknown = false;
        for member in ty.members() {
            match self.member_satisfies(member, expected) {
                Some(true) => any_yes = true,
                Some(false) => any_no = true,
                None => any_unknown = true,
            }
        }
        match (any_yes, any_no, any_unknown) {
            (true, false, false) => Some(true),
            (false, true, false) => Some(false),
            _ => None,
        }
    }

    fn member_satisfies(&self, member: &Type, expected: &Type) -> Option<bool> {
        let mut unknown = false;
        for candidate in expected.members() {
            match (member, candidate) {
                (Type::Unknown | Type::Mixed, _) => return None,
                (_, Type::Mixed) => return Some(true),
                (Type::Null, Type::Null) | (Type::Int | Type::IntLiteral(_), Type::Int) => return Some(true),
                (Type::String | Type::StringLiteral(_), Type::String) => return Some(true),
                (Type::Float, Type::Float) | (Type::Bool | Type::True | Type::False, Type::Bool) => {
                    return Some(true);
                }
                (Type::True, Type::True) | (Type::False, Type::False) => return Some(true),
                (
                    Type::Array(..) | Type::List(_) | Type::Shape(_),
                    Type::Array(..) | Type::List(_) | Type::Shape(_),
                ) => {
                    return Some(true);
                }
                (Type::Callable(_), Type::Callable(_)) => return Some(true),
                (Type::ClassString(Some(sub)), Type::ClassString(Some(sup))) => match self.satisfies(sub, sup) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => unknown = true,
                },
                (Type::ClassString(_), Type::ClassString(_) | Type::String) => return Some(true),
                (Type::String | Type::StringLiteral(_), Type::ClassString(_)) => unknown = true,
                (Type::Class { name: sub, .. }, Type::Class { name: sup, .. }) => {
                    if self.index.is_subclass_of(sub, sup) {
                        return Some(true);
                    }
                    if self.index.class(sub).is_none() || self.index.class(sup).is_none() {
                        unknown = true;
                    }
                }
                (Type::Class { .. } | Type::Static, Type::Object) => return Some(true),
                (Type::Template(_) | Type::Static | Type::SelfType | Type::Parent, _) | (_, Type::Template(_)) => {
                    unknown = true;
                }
                _ => {}
            }
        }
        if unknown { None } else { Some(false) }
    }

    /// Evaluates the conditional types inside a type given what is known of the arguments and the
    /// template bindings.
    pub fn resolve_conditionals(
        &self,
        ty: &Type,
        arguments: &HashMap<String, Type>,
        map: &HashMap<String, Type>,
    ) -> Type {
        match ty {
            Type::Conditional(conditional) => {
                let subject = if conditional.subject.starts_with('$') {
                    arguments.get(&conditional.subject).cloned()
                } else {
                    map.get(&conditional.subject).cloned()
                };
                let expected = conditional.is.substitute(map, None, None);
                let verdict = subject.as_ref().and_then(|subject| self.satisfies(subject, &expected));
                let verdict = verdict.map(|verdict| verdict != conditional.negated);
                let then = self.resolve_conditionals(&conditional.then, arguments, map);
                let otherwise = self.resolve_conditionals(&conditional.otherwise, arguments, map);
                match verdict {
                    Some(true) => then,
                    Some(false) => otherwise,
                    None => Type::union([then, otherwise]),
                }
            }
            Type::Union(members) => Type::union(
                members
                    .iter()
                    .map(|member| self.resolve_conditionals(member, arguments, map)),
            ),
            other => other.clone(),
        }
    }
}

/// A literal type is widened where it becomes the answer for a template, as `T` of `f(1)` is `int`.
fn widen(ty: &Type) -> Type {
    match ty {
        Type::IntLiteral(_) => Type::Int,
        Type::StringLiteral(_) => Type::String,
        Type::True | Type::False => Type::Bool,
        Type::Union(members) => Type::union(members.iter().map(widen)),
        other => other.clone(),
    }
}

/// The names of the templates a type still mentions.
pub fn template_names(ty: &Type, out: &mut Vec<String>) {
    match ty {
        Type::Template(name) => {
            if !out.contains(name) {
                out.push(name.clone());
            }
        }
        Type::Array(a, b) | Type::Iterable(a, b) => {
            template_names(a, out);
            template_names(b, out);
        }
        Type::List(inner) | Type::ClassString(Some(inner)) => template_names(inner, out),
        Type::Class { args, .. } => args.iter().for_each(|arg| template_names(arg, out)),
        Type::Union(members) | Type::Intersection(members) => {
            members.iter().for_each(|member| template_names(member, out))
        }
        Type::Shape(fields) => fields.iter().for_each(|field| template_names(&field.ty, out)),
        Type::Callable(Some(callable)) => {
            callable.params.iter().for_each(|param| template_names(&param.ty, out));
            if let Some(ret) = &callable.ret {
                template_names(ret, out);
            }
        }
        _ => {}
    }
}
