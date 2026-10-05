//! The members of a class as a caller sees them: its own, its traits', its parents' and its
//! interfaces', with the template arguments of `@extends Base<Foo>` carried down the chain.

use std::borrow::Cow;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::index::{Class, Index};
use crate::model::*;
use crate::types::{Name, Type};

const MAX_DEPTH: usize = 24;

#[derive(Clone, Debug)]
pub struct AliasRule {
    pub method: String,
    pub alias: Option<String>,
    pub visibility: Option<Visibility>,
}

#[derive(Clone)]
pub struct Ancestor<'a> {
    pub class: Class<'a>,
    /// The class's own template names bound to the types it was reached with.
    pub subst: Arc<HashMap<String, Type>>,
    /// What `self` is in its members: the class itself, or the class using a trait.
    pub self_name: Name,
    pub via_trait: bool,
    /// Reached through a `@mixin` of a doc comment, which lends members without being inherited.
    pub mixin: bool,
    /// Lowercase names of methods that `insteadof` takes away from this trait.
    pub excluded: Vec<String>,
    pub aliases: Vec<AliasRule>,
}

/// A member, with what is needed to read its types from where it was found.
#[derive(Clone)]
pub struct Found<'a, T: Clone> {
    pub class: Class<'a>,
    pub member: Cow<'a, T>,
    pub subst: Arc<HashMap<String, Type>>,
    pub self_name: Name,
    /// Found through a `@mixin`, so a call to it goes through the receiver's magic methods.
    pub mixin: bool,
    /// What `static` and `$this` stand for in the member when it is not the type it was found on: a
    /// call a model forwards to its builder returns the builder.
    pub static_as: Option<Type>,
}

impl<T: Clone> Found<'_, T> {
    /// A type of the member with its templates and `self` filled in. `static` stays, for the caller
    /// to bind to the receiver.
    pub fn resolve(&self, ty: &Type) -> Type {
        ty.substitute(&self.subst, None, Some(&self.self_name))
    }
}

impl Index {
    /// The class and everything above it, in the order PHP looks members up: the class, its traits,
    /// its parent chain, its interfaces, then the classes its doc comments mix in.
    pub fn ancestors(&self, ty: &Type) -> Vec<Ancestor<'_>> {
        let mut out = Vec::new();
        let mut seen = HashMap::new();
        match ty {
            Type::Class { .. } => self.walk(ty, false, None, Vec::new(), Vec::new(), &mut out, &mut seen, 0),
            Type::Union(members) | Type::Intersection(members) => {
                for member in members {
                    if matches!(member, Type::Class { .. }) {
                        self.walk(member, false, None, Vec::new(), Vec::new(), &mut out, &mut seen, 0);
                    }
                }
            }
            _ => {}
        }
        let mut mixins = Vec::new();
        for ancestor in &out {
            if let Some(doc) = &ancestor.class.decl.doc {
                for mixin in &doc.mixins {
                    mixins.push(mixin.substitute(&ancestor.subst, None, Some(&ancestor.self_name)));
                }
            }
        }
        let first_mixin = out.len();
        for mixin in mixins {
            self.walk(&mixin, false, None, Vec::new(), Vec::new(), &mut out, &mut seen, 0);
        }
        for ancestor in &mut out[first_mixin..] {
            ancestor.mixin = true;
        }
        if self.framework.frameworks.eloquent {
            crate::framework::eloquent::bind_factory_model(self, &mut out);
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn walk<'a>(
        &'a self,
        ty: &Type,
        via_trait: bool,
        using: Option<&str>,
        excluded: Vec<String>,
        aliases: Vec<AliasRule>,
        out: &mut Vec<Ancestor<'a>>,
        seen: &mut HashMap<String, (bool, usize)>,
        depth: usize,
    ) {
        let Type::Class { name, args } = ty else {
            return;
        };
        if depth > MAX_DEPTH {
            return;
        }
        let Some(class) = self.class(name) else {
            return;
        };
        // A class met again is walked again only when the first time left a template unbound (or gave
        // none at all) and this way down binds it, as a child that names the interface its raw parent
        // implements. It then keeps its place and takes the better answer.
        let wanted = class.decl.doc.iter().flat_map(|doc| doc.templates.iter()).count();
        let bound = args.len() >= wanted && args.iter().all(|arg| !arg.has_template());
        let revisited = match seen.entry(name.to_ascii_lowercase()) {
            Entry::Occupied(mut visited) => {
                if visited.get().0 || !bound {
                    return;
                }
                visited.get_mut().0 = true;
                Some(visited.get().1)
            }
            Entry::Vacant(vacant) => {
                vacant.insert((bound, out.len()));
                None
            }
        };
        let mut subst = HashMap::new();
        for (index, template) in class.decl.doc.iter().flat_map(|doc| doc.templates.iter()).enumerate() {
            if let Some(arg) = args.get(index) {
                subst.insert(template.name.clone(), arg.clone());
            } else if let Some(default) = &template.default {
                subst.insert(template.name.clone(), default.clone());
            }
        }
        let subst = Arc::new(subst);
        let self_name = using.map_or_else(|| class.decl.name.clone(), str::to_string);
        match revisited {
            Some(at) => out[at].subst = subst.clone(),
            None => out.push(Ancestor {
                class,
                subst: subst.clone(),
                self_name: self_name.clone(),
                via_trait,
                mixin: false,
                excluded,
                aliases,
            }),
        }

        for usage in &class.decl.trait_uses {
            let trait_type = usage.ty.substitute(&subst, None, None);
            let Type::Class { name: trait_name, .. } = &trait_type else {
                continue;
            };
            let mut excluded = Vec::new();
            let mut aliases = Vec::new();
            for adaptation in &usage.adaptations {
                match adaptation {
                    Adaptation::InsteadOf {
                        excluded: names,
                        method,
                        ..
                    } => {
                        if names.iter().any(|name| name.eq_ignore_ascii_case(trait_name)) {
                            excluded.push(method.to_ascii_lowercase());
                        }
                    }
                    Adaptation::Alias {
                        trait_name: from,
                        method,
                        alias,
                        visibility,
                    } => {
                        if from.as_ref().is_none_or(|from| from.eq_ignore_ascii_case(trait_name)) {
                            aliases.push(AliasRule {
                                method: method.to_ascii_lowercase(),
                                alias: alias.clone(),
                                visibility: *visibility,
                            });
                        }
                    }
                }
            }
            self.walk(
                &trait_type,
                true,
                Some(&self_name),
                excluded,
                aliases,
                out,
                seen,
                depth + 1,
            );
        }
        for parent in &class.decl.extends {
            self.walk(
                &parent.substitute(&subst, None, None),
                false,
                None,
                Vec::new(),
                Vec::new(),
                out,
                seen,
                depth + 1,
            );
        }
        // An `@implements` naming an interface the class only inherits is the way a child binds the
        // templates of a parent that is written raw.
        let documented = class
            .decl
            .doc
            .iter()
            .flat_map(|doc| doc.implements.iter())
            .filter(|documented| {
                !class
                    .decl
                    .implements
                    .iter()
                    .any(|declared| matches!((declared, documented), (Type::Class { name: left, .. }, Type::Class { name: right, .. }) if left.eq_ignore_ascii_case(right)))
            });
        for interface in class.decl.implements.iter().chain(documented) {
            self.walk(
                &interface.substitute(&subst, None, None),
                false,
                None,
                Vec::new(),
                Vec::new(),
                out,
                seen,
                depth + 1,
            );
        }
    }

    pub fn is_subclass_of(&self, child: &str, parent: &str) -> bool {
        if child.eq_ignore_ascii_case(parent) {
            return true;
        }
        self.ancestors(&Type::class(child))
            .iter()
            .any(|ancestor| ancestor.class.decl.name.eq_ignore_ascii_case(parent))
    }

    /// Whether code inside `context` (a class name, `None` outside any class) may use a member.
    pub fn is_accessible(&self, visibility: Visibility, declaring: &str, context: Option<&str>) -> bool {
        match visibility {
            Visibility::Public => true,
            Visibility::Protected => context.is_some_and(|context| {
                self.is_subclass_of(context, declaring) || self.is_subclass_of(declaring, context)
            }),
            Visibility::Private => context.is_some_and(|context| context.eq_ignore_ascii_case(declaring)),
        }
    }

    /// Every method the type has, the nearest declaration of each name.
    pub fn methods(&self, ty: &Type) -> Vec<Found<'_, Method>> {
        self.methods_named(ty, None, true)
    }

    /// The methods of a type, or only the one of this name, which skips the work of the others.
    fn methods_named(&self, ty: &Type, only: Option<&str>, framework: bool) -> Vec<Found<'_, Method>> {
        let ancestors = self.ancestors(ty);
        let mut out: Vec<Found<'_, Method>> = Vec::new();
        let mut names = HashSet::new();
        for ancestor in &ancestors {
            let decl = ancestor.class.decl;
            for method in &decl.methods {
                if !method.availability.contains(self.level) {
                    continue;
                }
                let lower = method.name.to_ascii_lowercase();
                if only.is_some_and(|only| {
                    !lower.eq_ignore_ascii_case(only) && ancestor.aliases.iter().all(|rule| rule.method != lower)
                }) {
                    continue;
                }
                for rule in ancestor.aliases.iter().filter(|rule| rule.method == lower) {
                    if let Some(alias) = &rule.alias {
                        let mut aliased = method.clone();
                        aliased.name = alias.clone();
                        if let Some(visibility) = rule.visibility {
                            aliased.visibility = visibility;
                        }
                        push_method(&mut out, &mut names, ancestor, Cow::Owned(aliased));
                    }
                }
                if ancestor.excluded.contains(&lower) {
                    continue;
                }
                let restyled = ancestor
                    .aliases
                    .iter()
                    .find(|rule| rule.method == lower && rule.alias.is_none())
                    .and_then(|rule| rule.visibility);
                match restyled {
                    Some(visibility) => {
                        let mut method = method.clone();
                        method.visibility = visibility;
                        push_method(&mut out, &mut names, ancestor, Cow::Owned(method));
                    }
                    None => push_method(&mut out, &mut names, ancestor, Cow::Borrowed(method)),
                }
            }
            if let Some(doc) = &decl.doc {
                for pseudo in &doc.methods {
                    if only.is_some_and(|only| !pseudo.name.eq_ignore_ascii_case(only)) {
                        continue;
                    }
                    push_method(&mut out, &mut names, ancestor, Cow::Owned(pseudo_method(pseudo, decl)));
                }
            }
        }
        if let Some(first) = ancestors
            .first()
            .filter(|first| first.class.decl.kind == ClassKind::Enum)
        {
            for synthetic in enum_methods(first.class.decl) {
                push_method(&mut out, &mut names, first, Cow::Owned(synthetic));
            }
        }
        if framework && self.framework.frameworks.any() {
            crate::framework::extend_methods(self, &ancestors, only, &mut out, &mut names);
        }
        out
    }

    pub fn find_method(&self, ty: &Type, name: &str) -> Option<Found<'_, Method>> {
        self.methods_named(ty, Some(name), true)
            .into_iter()
            .find(|found| found.member.name.eq_ignore_ascii_case(name))
    }

    /// A method the code declares, without the ones the frameworks make up at run time.
    pub fn find_declared_method(&self, ty: &Type, name: &str) -> Option<Found<'_, Method>> {
        self.methods_named(ty, Some(name), false)
            .into_iter()
            .find(|found| found.member.name.eq_ignore_ascii_case(name))
    }

    pub fn properties(&self, ty: &Type) -> Vec<Found<'_, Property>> {
        self.properties_named(ty, None)
    }

    fn properties_named(&self, ty: &Type, only: Option<&str>) -> Vec<Found<'_, Property>> {
        let ancestors = self.ancestors(ty);
        let mut out = Vec::new();
        let mut names = HashSet::new();
        for ancestor in &ancestors {
            let decl = ancestor.class.decl;
            for property in &decl.properties {
                if only.is_some_and(|only| property.name != only) {
                    continue;
                }
                if property.availability.contains(self.level) && names.insert(property.name.clone()) {
                    out.push(found_in(ancestor, Cow::Borrowed(property)));
                }
            }
            if let Some(doc) = &decl.doc {
                for pseudo in &doc.properties {
                    if names.insert(pseudo.name.clone()) {
                        out.push(found_in(ancestor, Cow::Owned(pseudo_property(pseudo, decl))));
                    }
                }
            }
            if decl.kind == ClassKind::Enum && !ancestor.via_trait {
                for property in enum_properties(decl) {
                    if names.insert(property.name.clone()) {
                        out.push(found_in(ancestor, Cow::Owned(property)));
                    }
                }
            }
        }
        if self.framework.frameworks.any() {
            crate::framework::extend_properties(self, &ancestors, only, &mut out, &mut names);
        }
        out
    }

    pub fn find_property(&self, ty: &Type, name: &str) -> Option<Found<'_, Property>> {
        self.properties_named(ty, Some(name))
            .into_iter()
            .find(|found| found.member.name == name)
    }

    pub fn constants_of(&self, ty: &Type) -> Vec<Found<'_, ClassConst>> {
        let ancestors = self.ancestors(ty);
        let mut out = Vec::new();
        let mut names = HashSet::new();
        for ancestor in &ancestors {
            for constant in &ancestor.class.decl.constants {
                if constant.availability.contains(self.level) && names.insert(constant.name.clone()) {
                    out.push(found_in(ancestor, Cow::Borrowed(constant)));
                }
            }
        }
        out
    }

    pub fn find_constant(&self, ty: &Type, name: &str) -> Option<Found<'_, ClassConst>> {
        self.constants_of(ty)
            .into_iter()
            .find(|found| found.member.name == name)
    }
}

fn found_in<'a, T: Clone>(ancestor: &Ancestor<'a>, member: Cow<'a, T>) -> Found<'a, T> {
    Found {
        class: ancestor.class,
        member,
        subst: ancestor.subst.clone(),
        self_name: ancestor.self_name.clone(),
        mixin: ancestor.mixin,
        static_as: None,
    }
}

fn push_method<'a>(
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
    ancestor: &Ancestor<'a>,
    method: Cow<'a, Method>,
) {
    if names.insert(method.name.to_ascii_lowercase()) {
        out.push(Found {
            class: ancestor.class,
            member: method,
            subst: ancestor.subst.clone(),
            self_name: ancestor.self_name.clone(),
            mixin: ancestor.mixin,
            static_as: None,
        });
    }
}

fn pseudo_method(pseudo: &DocMethod, class: &ClassDecl) -> Method {
    Method {
        name: pseudo.name.clone(),
        visibility: Visibility::Public,
        is_static: pseudo.is_static,
        is_abstract: false,
        is_final: false,
        callable: Callable {
            params: pseudo
                .params
                .iter()
                .map(|param| Param {
                    name: param.name.clone(),
                    ty: None,
                    doc_ty: param.ty.clone(),
                    leveled: None,
                    default: None,
                    variadic: param.variadic,
                    by_ref: param.by_ref,
                    promoted: None,
                    description: param.description.clone(),
                    attributes: Vec::new(),
                    availability: Availability::default(),
                    span: class.name_span,
                })
                .collect(),
            ret: None,
            doc_ret: pseudo.ret.clone(),
            leveled_ret: None,
            by_ref_return: false,
            is_generator: false,
            reads_all_arguments: false,
        },
        doc: (!pseudo.description.is_empty()).then(|| {
            Box::new(Doc {
                summary: pseudo.description.clone(),
                ..Doc::default()
            })
        }),
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: class.name_span,
        span: class.span,
    }
}

fn pseudo_property(pseudo: &DocProperty, class: &ClassDecl) -> Property {
    Property {
        name: pseudo.name.clone(),
        visibility: Visibility::Public,
        set_visibility: None,
        is_static: false,
        is_readonly: pseudo.read_only,
        is_abstract: false,
        ty: None,
        doc_ty: pseudo.ty.clone(),
        leveled: None,
        default: None,
        promoted: false,
        hooks: Vec::new(),
        doc: (!pseudo.description.is_empty()).then(|| {
            Box::new(Doc {
                summary: pseudo.description.clone(),
                ..Doc::default()
            })
        }),
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: class.name_span,
        span: class.span,
    }
}

fn enum_properties(decl: &ClassDecl) -> Vec<Property> {
    let make = |name: &str, ty: Type| Property {
        name: name.to_string(),
        visibility: Visibility::Public,
        set_visibility: None,
        is_static: false,
        is_readonly: true,
        is_abstract: false,
        ty: Some(ty),
        doc_ty: None,
        leveled: None,
        default: None,
        promoted: false,
        hooks: Vec::new(),
        doc: None,
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: decl.name_span,
        span: decl.span,
    };
    let mut out = vec![make("name", Type::String)];
    if let Some(backing) = &decl.backing {
        out.push(make("value", backing.clone()));
    }
    out
}

fn enum_methods(decl: &ClassDecl) -> Vec<Method> {
    let make = |name: &str, params: Vec<Param>, ret: Type| Method {
        name: name.to_string(),
        visibility: Visibility::Public,
        is_static: true,
        is_abstract: false,
        is_final: false,
        callable: Callable {
            params,
            ret: Some(ret),
            doc_ret: None,
            leveled_ret: None,
            by_ref_return: false,
            is_generator: false,
            reads_all_arguments: false,
        },
        doc: None,
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: decl.name_span,
        span: decl.span,
    };
    let own = Type::class(decl.name.clone());
    let mut out = vec![make("cases", Vec::new(), Type::List(Box::new(own.clone())))];
    if decl.backing.is_some() {
        let value = Param {
            name: "value".to_string(),
            ty: Some(Type::union([Type::Int, Type::String])),
            doc_ty: None,
            leveled: None,
            default: None,
            variadic: false,
            by_ref: false,
            promoted: None,
            description: String::new(),
            attributes: Vec::new(),
            availability: Availability::default(),
            span: decl.name_span,
        };
        out.push(make("from", vec![value.clone()], own.clone()));
        out.push(make("tryFrom", vec![value], own.nullable()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ExtractOptions, extract};
    use crate::index::Origin;
    use php_syntax::{PhpVersion, parse};
    use std::path::PathBuf;

    fn index(text: &str) -> Index {
        let mut index = Index::new(PhpVersion::V8_4);
        index.set_file(
            PathBuf::from("/a.php"),
            Origin::Project,
            Arc::new(extract(&parse(text).syntax(), ExtractOptions::default())),
        );
        index
    }

    #[test]
    fn walks_parents_traits_and_interfaces_in_order() {
        let index = index(
            r#"<?php
interface I { public function fromInterface(); }
trait T { public function fromTrait() {} private function hidden() {} }
class P { public function fromParent() {} public function over() {} }
class C extends P implements I { use T; public function own() {} public function over() {} }
"#,
        );
        let names: Vec<String> = index
            .ancestors(&Type::class("C"))
            .iter()
            .map(|ancestor| ancestor.class.decl.name.clone())
            .collect();
        assert_eq!(names, vec!["C", "T", "P", "I"]);
        let methods: Vec<String> = index
            .methods(&Type::class("C"))
            .iter()
            .map(|found| format!("{}::{}", found.class.decl.name, found.member.name))
            .collect();
        assert_eq!(
            methods,
            vec![
                "C::own",
                "C::over",
                "T::fromTrait",
                "T::hidden",
                "P::fromParent",
                "I::fromInterface"
            ]
        );
    }

    #[test]
    fn carries_template_arguments_down_the_chain() {
        let index = index(
            r#"<?php
/** @template T */
class Box { /** @return T */ public function get() {} /** @var T */ public $value; }
/** @extends Box<User> */
class UserBox extends Box {}
class User {}
"#,
        );
        let found = index.find_method(&Type::class("UserBox"), "get").unwrap();
        let ret = found.member.callable.doc_ret.clone().unwrap();
        assert_eq!(found.resolve(&ret), Type::class("User"));
        let property = index.find_property(&Type::class("UserBox"), "value").unwrap();
        assert_eq!(
            found.resolve(property.member.doc_ty.as_ref().unwrap()),
            Type::class("User")
        );
        let direct = Type::Class {
            name: "Box".into(),
            args: vec![Type::Int],
        };
        let found = index.find_method(&direct, "get").unwrap();
        assert_eq!(
            found.resolve(&found.member.callable.doc_ret.clone().unwrap()),
            Type::Int
        );
    }

    #[test]
    fn applies_trait_adaptations() {
        let index = index(
            r#"<?php
trait A { public function hello() {} public function other() {} }
trait B { public function hello() {} }
class C { use A, B { A::hello insteadof B; B::hello as protected greet; other as private; } }
"#,
        );
        let methods = index.methods(&Type::class("C"));
        let hello = methods.iter().find(|found| found.member.name == "hello").unwrap();
        assert_eq!(hello.class.decl.name, "A");
        let greet = methods.iter().find(|found| found.member.name == "greet").unwrap();
        assert_eq!(greet.member.visibility, Visibility::Protected);
        let other = methods.iter().find(|found| found.member.name == "other").unwrap();
        assert_eq!(other.member.visibility, Visibility::Private);
    }

    #[test]
    fn reads_doc_pseudo_members_and_mixins() {
        let index = index(
            r#"<?php
class Helper { public function help() {} }
/**
 * @property string $magic
 * @method static self make()
 * @mixin Helper
 */
class Model {}
"#,
        );
        assert!(index.find_property(&Type::class("Model"), "magic").is_some());
        assert!(
            index
                .find_method(&Type::class("Model"), "make")
                .unwrap()
                .member
                .is_static
        );
        assert!(index.find_method(&Type::class("Model"), "help").is_some());
    }

    #[test]
    fn enums_have_cases_and_name() {
        let index = index("<?php enum Suit: string { case Hearts = 'h'; }");
        let ty = Type::class("Suit");
        assert!(index.find_method(&ty, "cases").is_some());
        assert!(index.find_method(&ty, "tryFrom").is_some());
        assert!(index.find_property(&ty, "value").is_some());
        assert!(index.find_constant(&ty, "Hearts").unwrap().member.is_case);
    }

    #[test]
    fn checks_visibility_from_a_context() {
        let index = index("<?php class P {} class C extends P {} class Other {}");
        assert!(index.is_accessible(Visibility::Protected, "P", Some("C")));
        assert!(index.is_accessible(Visibility::Protected, "C", Some("P")));
        assert!(!index.is_accessible(Visibility::Protected, "P", Some("Other")));
        assert!(!index.is_accessible(Visibility::Private, "P", Some("C")));
        assert!(index.is_accessible(Visibility::Private, "P", Some("p")));
        assert!(!index.is_accessible(Visibility::Protected, "P", None));
    }

    #[test]
    fn survives_cycles() {
        let index = index("<?php class A extends B {} class B extends A {}");
        assert_eq!(index.ancestors(&Type::class("A")).len(), 2);
    }
}
