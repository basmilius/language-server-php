//! Classes: the abstract and interface methods a concrete class leaves out, methods that cannot
//! take the place of the ones they override, and the file-level `declare` a file lacks.

use std::collections::HashSet;

use php_index::{ClassKind, Method, Origin, Param, Type, Visibility};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::types::{Kinds, kinds_of};
use super::{Cx, Fix};
use crate::ast::{child_of, text_of};

pub(super) fn run(cx: &Cx) {
    for node in &cx.nodes {
        if matches!(node.kind(), CLASS_DECLARATION | ENUM_DECLARATION) {
            check_class(cx, node);
        }
    }
    check_strict_types(cx);
}

fn check_class(cx: &Cx, node: &SyntaxNode) {
    let Some(name_node) = child_of(node, NAME) else {
        return;
    };
    let analyzer = cx.file.analyzer(node);
    let name = analyzer.resolver.qualify(&text_of(&name_node));
    let Some(class) = cx.index.class(&name) else {
        return;
    };
    if cx.index.class_declarations(&name) != 1 || !cx.hierarchy_complete(&name) {
        return;
    }
    if class.decl.kind == ClassKind::Class && !class.decl.is_abstract || class.decl.kind == ClassKind::Enum {
        check_missing_methods(cx, node, &name_node, &name);
    }
    if cx.on("incompatible-override") {
        check_overrides(cx, node, &name);
    }
}

// Missing implementations -----------------------------------------------------------------------

/// A method a concrete class has to implement and does not.
pub(crate) struct Missing {
    pub method: Method,
    pub owner: String,
    pub from_interface: bool,
}

/// The abstract and interface methods nothing in a class's hierarchy implements.
pub(crate) fn missing_methods(cx: &Cx, name: &str) -> Vec<Missing> {
    let ty = Type::class(name.to_string());
    let level = cx.index.level;
    let mut missing: Vec<Missing> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let ancestors = cx.index.ancestors(&ty);
    for ancestor in ancestors.iter().skip(1).filter(|ancestor| !ancestor.mixin) {
        let decl = ancestor.class.decl;
        let from_interface = decl.kind == ClassKind::Interface;
        for method in &decl.methods {
            let lower = method.name.to_ascii_lowercase();
            let needs_body = method.is_abstract || from_interface;
            if !needs_body
                || method.visibility == Visibility::Private
                || !method.availability.contains(level)
                || ancestor.excluded.contains(&lower)
                || !seen.insert(lower)
            {
                continue;
            }
            let implemented =
                ancestors.iter().filter(|other| !other.mixin).any(|other| {
                    other.class.decl.kind != ClassKind::Interface
                        && !other.excluded.contains(&method.name.to_ascii_lowercase())
                        && other.class.decl.method(&method.name).is_some_and(|candidate| {
                            !candidate.is_abstract && candidate.visibility != Visibility::Private
                        })
                }) || enum_provides(&ancestors, &method.name);
            if !implemented {
                missing.push(Missing {
                    method: method.clone(),
                    owner: decl.name.clone(),
                    from_interface,
                });
            }
        }
    }
    missing
}

fn check_missing_methods(cx: &Cx, node: &SyntaxNode, name_node: &SyntaxNode, name: &str) {
    if !(cx.on("abstract-method-not-implemented") || cx.on("interface-method-not-implemented")) {
        return;
    }
    let missing = missing_methods(cx, name);
    let fix = Fix::ImplementMembers {
        class: crate::ast::start(node),
    };
    for (code, from_interface, noun) in [
        ("abstract-method-not-implemented", false, "abstract"),
        ("interface-method-not-implemented", true, "interface"),
    ] {
        let labels: Vec<String> = missing
            .iter()
            .filter(|entry| entry.from_interface == from_interface)
            .map(|entry| format!("{}::{}()", entry.owner, entry.method.name))
            .collect();
        if labels.is_empty() {
            continue;
        }
        let shown: Vec<&str> = labels.iter().take(3).map(String::as_str).collect();
        let more = if labels.len() > 3 {
            format!(" and {} more", labels.len() - 3)
        } else {
            String::new()
        };
        cx.report(
            code,
            name_node.text_range(),
            format!(
                "'{}' must implement the {noun} method(s) {}{more}",
                crate::short(name),
                shown.join(", ")
            ),
            fix.clone(),
        );
    }
}

/// What the engine adds to an enum: `cases`, and `from` and `tryFrom` of a backed one.
fn enum_provides(ancestors: &[php_index::Ancestor<'_>], method: &str) -> bool {
    let is_enum = ancestors
        .first()
        .is_some_and(|own| own.class.decl.kind == ClassKind::Enum);
    let backed = ancestors.first().is_some_and(|own| own.class.decl.backing.is_some());
    let lower = method.to_ascii_lowercase();
    is_enum && (lower == "cases" || backed && (lower == "from" || lower == "tryfrom"))
}

// Overrides -------------------------------------------------------------------------------------

fn check_overrides(cx: &Cx, class_node: &SyntaxNode, class_name: &str) {
    let Some(body) = child_of(class_node, CLASS_BODY) else {
        return;
    };
    let ty = Type::class(class_name.to_string());
    let ancestors = cx.index.ancestors(&ty);
    let Some(class) = cx.index.class(class_name) else {
        return;
    };
    for member in body.children().filter(|member| member.kind() == METHOD_DECLARATION) {
        let Some(name_node) = child_of(&member, NAME) else {
            continue;
        };
        let name = text_of(&name_node);
        let Some(own) = class.decl.method(&name) else {
            continue;
        };
        for ancestor in ancestors.iter().skip(1).filter(|ancestor| !ancestor.mixin) {
            let lower = name.to_ascii_lowercase();
            let Some(parent) = ancestor.class.decl.method(&name) else {
                continue;
            };
            if parent.visibility == Visibility::Private
                || ancestor.via_trait
                || ancestor.excluded.contains(&lower)
                || !parent.availability.contains(cx.index.level)
            {
                continue;
            }
            let parent_abstract = parent.is_abstract || ancestor.class.decl.kind == ClassKind::Interface;
            if lower == "__construct" && !parent_abstract {
                continue;
            }
            let from_stub = ancestor.class.file.origin == Origin::Stub;
            if let Some(problem) = incompatibility(cx, own, parent, from_stub) {
                cx.report(
                    "incompatible-override",
                    name_node.text_range(),
                    format!(
                        "'{}::{}' cannot override '{}::{}': {problem}",
                        crate::short(class_name),
                        own.name,
                        ancestor.class.decl.name,
                        parent.name
                    ),
                    visibility_fix(&member, own, parent),
                );
                break;
            }
        }
    }
}

fn visibility_fix(member: &SyntaxNode, own: &Method, parent: &Method) -> Fix {
    if rank(own.visibility) <= rank(parent.visibility) {
        return Fix::None;
    }
    match child_of(member, MODIFIER_LIST) {
        Some(modifiers) => Fix::Visibility {
            member: modifiers.text_range(),
            to: parent.visibility.keyword(),
        },
        None => Fix::None,
    }
}

fn rank(visibility: Visibility) -> u8 {
    match visibility {
        Visibility::Public => 0,
        Visibility::Protected => 1,
        Visibility::Private => 2,
    }
}

/// What makes a method unable to stand in for the one it overrides, when something does.
fn incompatibility(cx: &Cx, own: &Method, parent: &Method, parent_is_internal: bool) -> Option<String> {
    let level = cx.index.level;
    if parent.is_final {
        return Some("the parent method is final".to_string());
    }
    if parent.is_static != own.is_static {
        return Some(if parent.is_static {
            "the parent method is static".to_string()
        } else {
            "the parent method is not static".to_string()
        });
    }
    if rank(own.visibility) > rank(parent.visibility) {
        return Some(format!(
            "it must be {} as the parent method is",
            parent.visibility.keyword()
        ));
    }
    let own_params: Vec<&Param> = own.callable.params_at(level).collect();
    let parent_params: Vec<&Param> = parent.callable.params_at(level).collect();
    let own_variadic = own_params.iter().any(|param| param.variadic);
    let parent_variadic = parent_params.iter().any(|param| param.variadic);
    let own_required = own_params.iter().filter(|param| !param.is_optional()).count();
    let parent_required = parent_params.iter().filter(|param| !param.is_optional()).count();
    if own_required > parent_required {
        return Some(format!(
            "it requires {own_required} arguments, the parent method {parent_required}"
        ));
    }
    if !parent_variadic && !own_variadic && own_params.len() < parent_params.len() {
        return Some(format!(
            "it takes {} arguments, the parent method {}",
            own_params.len(),
            parent_params.len()
        ));
    }
    for (own_param, parent_param) in own_params.iter().zip(&parent_params) {
        if own_param.by_ref != parent_param.by_ref {
            return Some(format!(
                "parameter ${} differs in being taken by reference",
                own_param.name
            ));
        }
        let wanted = parent_param.native_type(level);
        let accepted = own_param.native_type(level);
        let verdict = match (accepted, wanted) {
            (None, _) => Some(true),
            (Some(accepted), None) => is_subtype(cx, &Type::Mixed, accepted),
            (Some(accepted), Some(wanted)) => is_subtype(cx, wanted, accepted),
        };
        if verdict == Some(false) {
            return Some(format!("parameter ${} is narrower than the parent's", own_param.name));
        }
    }
    let parent_return = parent.callable.native_return(level);
    let own_return = own.callable.native_return(level);
    match (own_return, parent_return) {
        (None, Some(Type::Void | Type::Mixed)) | (_, None) => {}
        (None, Some(_)) if parent_is_internal => {}
        (None, Some(_)) => return Some("the parent method declares a return type".to_string()),
        (Some(own_return), Some(parent_return)) => {
            if !parent_is_internal && is_subtype(cx, own_return, parent_return) == Some(false) {
                return Some(format!(
                    "its return type '{}' is not '{}'",
                    own_return.display(true),
                    parent_return.display(true)
                ));
            }
        }
    }
    None
}

/// Whether every value of one type is a value of the other, when that is certain either way.
fn is_subtype(cx: &Cx, sub: &Type, sup: &Type) -> Option<bool> {
    let mut unsure = false;
    for member in sub.members() {
        let mut accepted = false;
        let mut maybe = false;
        for candidate in sup.members() {
            match member_subtype(cx, member, candidate) {
                Some(true) => {
                    accepted = true;
                    break;
                }
                None => maybe = true,
                Some(false) => {}
            }
        }
        if accepted {
            continue;
        }
        if maybe {
            unsure = true;
            continue;
        }
        return Some(false);
    }
    if unsure { None } else { Some(true) }
}

fn member_subtype(cx: &Cx, sub: &Type, sup: &Type) -> Option<bool> {
    match (sub, sup) {
        (_, Type::Mixed) | (Type::Never, _) => Some(true),
        (Type::Mixed, Type::Static | Type::SelfType | Type::Parent | Type::Template(_)) => None,
        (Type::Mixed, _) => Some(false),
        (Type::Unknown, _) | (_, Type::Unknown) => None,
        (Type::Class { name: left, .. }, Type::Class { name: right, .. }) => {
            if cx.index.is_subclass_of(left, right) {
                return Some(true);
            }
            if cx.hierarchy_complete(left) && cx.hierarchy_complete(right) {
                // A name that did not resolve to a class at all says nothing.
                Some(false)
            } else {
                None
            }
        }
        (Type::Class { .. }, Type::Object) | (Type::Object, Type::Object) => Some(true),
        (Type::True | Type::False, Type::Bool) => Some(true),
        (Type::IntLiteral(_), Type::Int) | (Type::StringLiteral(_), Type::String) => Some(true),
        (Type::Static | Type::SelfType | Type::Parent | Type::Template(_), _)
        | (_, Type::Static | Type::SelfType | Type::Parent | Type::Template(_)) => None,
        _ => {
            let (left, right) = (kinds_of(sub)?, kinds_of(sup)?);
            if !left.intersects(right) {
                return Some(false);
            }
            if left == right && left != Kinds::OBJECT {
                return Some(true);
            }
            None
        }
    }
}

// Strict types ----------------------------------------------------------------------------------

fn check_strict_types(cx: &Cx) {
    if !cx.on("missing-strict-types") || cx.strict_types {
        return;
    }
    let has_code = cx.root.children().any(|child| {
        !matches!(
            child.kind(),
            DECLARE_STATEMENT | USE_STATEMENT | EMPTY_STATEMENT | ERROR
        )
    });
    if !has_code {
        return;
    }
    let Some(open) = cx
        .root
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == OPEN_TAG)
    else {
        return;
    };
    if cx
        .root
        .descendants_with_tokens()
        .any(|element| element.kind() == INLINE_HTML && !element.to_string().trim().is_empty())
    {
        return;
    }
    cx.report(
        "missing-strict-types",
        open.text_range(),
        "This file does not declare strict_types=1".to_string(),
        Fix::AddStrictTypes,
    );
}
