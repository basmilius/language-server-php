//! Members: methods, properties and class constants that a class does not have, that are
//! deprecated, that sit on the wrong side of static, or that an enum or `readonly` forbids.

use php_index::{ClassKind, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::names::{deprecated_message, deprecation, inside_deprecated};
use super::types::{Kinds, kinds_of};
use super::util::{
    guarded_by_check, in_guard, is_callee, is_relative_class, member_name, reassigned_in_loop, root_variable,
};
use super::{Cx, Fix};
use crate::ast::{self, text_of};
use crate::infer::{Analyzer, Env};
use crate::refs::is_write_target;

pub(super) fn run(cx: &Cx) {
    for node in &cx.nodes {
        match node.kind() {
            PROPERTY_FETCH_EXPR => instance_access(cx, node),
            SCOPED_ACCESS_EXPR => scoped_access(cx, node),
            STATIC_PROPERTY_EXPR => static_property(cx, node),
            NEW_EXPR => new_of_enum(cx, node),
            _ => {}
        }
    }
}

/// The classes an expression's value is an instance of, when all of them are known.
fn classes_of(analyzer: &Analyzer<'_>, cx: &Cx, ty: &Type) -> Option<(Type, Vec<String>)> {
    let receiver = analyzer.receiver_type(ty);
    let classes = cx.known_classes(&receiver)?;
    Some((receiver, classes))
}

/// Whether the type layer's answer for the object of an access cannot be trusted: a loop changes
/// it, an `instanceof` somewhere in the function narrows it, or `$this` is not the class around.
pub(super) fn unreliable_receiver(cx: &Cx, node: &SyntaxNode, object: &SyntaxNode) -> bool {
    if reassigned_in_loop(object) || is_narrowed_by_instanceof(object) {
        return true;
    }
    let is_this = root_variable(object).is_some_and(|variable| text_of(&variable) == "$this");
    // Pest's test case takes what `beforeEach` sets and what its own `__call` forwards.
    let in_pest = text_of(object) == "$this" && cx.file.analyzer(node).pest.is_some();
    is_this && (in_anonymous_class(node) || in_rebindable_closure(cx, node)) || in_pest
}

/// Whether `$this` here may be another object: a closure that the function it is made in binds
/// somewhere else, with `call`, `bindTo` or `Closure::bind`.
fn in_rebindable_closure(cx: &Cx, node: &SyntaxNode) -> bool {
    let Some(closure) = node
        .ancestors()
        .find(|ancestor| matches!(ancestor.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
    else {
        return false;
    };
    let scope = closure
        .ancestors()
        .skip(1)
        .find(|ancestor| ast::is_function_like(ancestor.kind()) || ancestor.kind() == SOURCE_FILE)
        .unwrap_or_else(|| cx.root.clone());
    let text = scope.text().to_string();
    ["bindTo(", "->call(", "Closure::bind(", "Closure::fromCallable("]
        .iter()
        .any(|binder| text.contains(binder))
}

fn in_anonymous_class(node: &SyntaxNode) -> bool {
    node.ancestors().any(|ancestor| ancestor.kind() == ANONYMOUS_CLASS)
}

fn instance_access(cx: &Cx, node: &SyntaxNode) {
    let (Some(object), Some(name)) = (node.children().next(), member_name(node)) else {
        return;
    };
    if unreliable_receiver(cx, node, &object) {
        return;
    }
    let is_this = root_variable(&object).is_some_and(|variable| text_of(&variable) == "$this");
    let analyzer = cx.file.analyzer(node);
    let env = analyzer.env_around(node);
    let ty = analyzer.type_of(&object, &env);
    let Some((receiver, classes)) = classes_of(&analyzer, cx, &ty) else {
        return;
    };
    let written = text_of(&name);
    if is_callee(node) {
        check_method(cx, node, &name, &written, &receiver, &classes, false, is_this);
    } else {
        check_property(cx, node, &name, &written, &receiver, &classes, is_this);
    }
}

#[allow(clippy::too_many_arguments)]
fn check_method(
    cx: &Cx,
    access: &SyntaxNode,
    name: &SyntaxNode,
    written: &str,
    receiver: &Type,
    classes: &[String],
    statically: bool,
    is_this: bool,
) {
    let found: Vec<_> = receiver
        .members()
        .iter()
        .filter_map(|member| cx.index.find_method(member, written))
        .collect();
    if found.is_empty() {
        if !cx.on("undefined-method")
            || classes
                .iter()
                .any(|class| cx.has_magic(class, &["__call", "__callStatic"]))
            || guarded_by_check(access, &["method_exists(", "is_callable("], written)
            || classes.iter().any(|class| a_subtype_declares(cx, class, written, true))
        {
            return;
        }
        let class = &classes[0];
        if let Some(message) = enum_message(cx, class, written) {
            cx.report("enum-misuse", name.text_range(), message, Fix::None);
            return;
        }
        let fix = Fix::CreateMember {
            class: class.clone(),
            name: written.to_string(),
            is_static: statically,
            is_method: true,
            at: ast::start(access),
        };
        cx.report(
            "undefined-method",
            name.text_range(),
            format!("Method '{written}' is not declared in {}", classes.join("|")),
            fix,
        );
        return;
    }
    let first = &found[0];
    if cx.on("deprecated") && !inside_deprecated(access) {
        if let Some(reason) = deprecation(cx.index, first.member.doc.as_deref(), &first.member.attributes) {
            let label = format!("{}::{}", first.class.decl.name, first.member.name);
            cx.report(
                "deprecated",
                name.text_range(),
                deprecated_message("Method", &label, &reason),
                Fix::None,
            );
        }
    }
    // A method the class does not declare itself (a scope of a model, a `@method`) is reached
    // through `__call` on an object, which is no static call.
    let declared = found
        .iter()
        .all(|found| found.class.decl.method(&found.member.name).is_some());
    if found.iter().all(|found| found.member.is_static)
        && declared
        && !statically
        && !is_this
        && cx.on("instance-call-of-static-method")
    {
        let label = format!("{}::{}", first.class.decl.name, first.member.name);
        cx.report(
            "instance-call-of-static-method",
            name.text_range(),
            format!("Static method '{label}' is called through an object"),
            Fix::None,
        );
    }
}

fn enum_message(cx: &Cx, class: &str, member: &str) -> Option<String> {
    let found = cx.index.class(class)?;
    if found.decl.kind != ClassKind::Enum || found.decl.backing.is_some() {
        return None;
    }
    let lower = member.to_ascii_lowercase();
    if lower == "from" || lower == "tryfrom" {
        return Some(format!("Enum '{class}' is not backed, so it has no {member}()"));
    }
    None
}

fn check_property(
    cx: &Cx,
    access: &SyntaxNode,
    name: &SyntaxNode,
    written: &str,
    receiver: &Type,
    classes: &[String],
    is_this: bool,
) {
    let found: Vec<_> = receiver
        .members()
        .iter()
        .filter_map(|member| cx.index.find_property(member, written))
        .collect();
    let writing = is_write_target(access);
    if found.is_empty() {
        report_undefined_property(cx, access, name, written, classes, writing, is_this);
        return;
    }
    let first = &found[0];
    if cx.on("deprecated") && !inside_deprecated(access) {
        if let Some(reason) = deprecation(cx.index, first.member.doc.as_deref(), &first.member.attributes) {
            let label = format!("{}::${}", first.class.decl.name, first.member.name);
            cx.report(
                "deprecated",
                name.text_range(),
                deprecated_message("Property", &label, &reason),
                Fix::None,
            );
        }
    }
    if writing && cx.on("readonly-reassigned") && first.member.is_readonly {
        let declared = first.member.effective_type(cx.index.level);
        check_readonly_write(
            cx,
            access,
            name,
            &first.class.decl.name,
            &first.member.name,
            first.member.promoted,
            declared,
        );
    }
}

fn report_undefined_property(
    cx: &Cx,
    access: &SyntaxNode,
    name: &SyntaxNode,
    written: &str,
    classes: &[String],
    writing: bool,
    is_this: bool,
) {
    if !cx.on("undefined-property")
        || in_guard(access)
        || guarded_by_check(access, &["property_exists(", "isset("], written)
    {
        return;
    }
    for class in classes {
        // The standard library's stubs do not list what the engine adds, and what is dynamic is theirs.
        if !cx.is_user_class(class)
            || is_interface(cx, class)
            || cx.has_magic(class, &["__get", "__set"])
            || is_dynamic_class(cx, class)
            || a_subtype_declares(cx, class, written, false)
        {
            return;
        }
    }
    let class = &classes[0];
    if let Some(found) = cx.index.class(class) {
        if found.decl.kind == ClassKind::Enum && written == "value" {
            cx.report(
                "enum-misuse",
                name.text_range(),
                format!("Enum '{class}' is not backed, so its cases have no value"),
                Fix::None,
            );
            return;
        }
    }
    if writing {
        if cx.index.level < php_syntax::PhpVersion::V8_2 {
            return;
        }
        cx.report(
            "undefined-property",
            name.text_range(),
            format!(
                "Property '{written}' is not declared in {}, and dynamic properties are deprecated",
                classes.join("|")
            ),
            Fix::CreateMember {
                class: class.clone(),
                name: written.to_string(),
                is_static: false,
                is_method: false,
                at: ast::start(access),
            },
        );
        return;
    }
    let _ = is_this;
    cx.report(
        "undefined-property",
        name.text_range(),
        format!("Property '{written}' is not declared in {}", classes.join("|")),
        Fix::CreateMember {
            class: class.clone(),
            name: written.to_string(),
            is_static: false,
            is_method: false,
            at: ast::start(access),
        },
    );
}

fn is_interface(cx: &Cx, class: &str) -> bool {
    cx.index
        .class(class)
        .is_some_and(|found| found.decl.kind == ClassKind::Interface)
}

/// Whether a class below this one declares the member. A value typed as the class may be one of
/// those, and the type layer does not follow every `instanceof` that tells.
fn a_subtype_declares(cx: &Cx, class: &str, member: &str, method: bool) -> bool {
    cx.index.all_subtypes(class).iter().any(|subtype| {
        let ty = Type::class(subtype.decl.name.clone());
        if method {
            cx.index.find_method(&ty, member).is_some()
        } else {
            cx.index.find_property(&ty, member).is_some()
        }
    })
}

fn a_subtype_declares_constant(cx: &Cx, class: &str, name: &str) -> bool {
    cx.index.all_subtypes(class).iter().any(|subtype| {
        cx.index
            .find_constant(&Type::class(subtype.decl.name.clone()), name)
            .is_some()
    })
}

/// Whether an expression's root variable is tested with `instanceof` anywhere in its function.
fn is_narrowed_by_instanceof(object: &SyntaxNode) -> bool {
    let Some(variable) = root_variable(object) else {
        return false;
    };
    let name = text_of(&variable);
    let scope = object
        .ancestors()
        .find(|ancestor| ast::is_function_like(ancestor.kind()) || ancestor.kind() == SOURCE_FILE);
    let Some(scope) = scope else {
        return false;
    };
    scope.descendants().any(|node| {
        node.kind() == BINARY_EXPR
            && ast::tokens(&node).any(|token| token.kind() == INSTANCEOF_KW)
            && node.children().next().is_some_and(|left| text_of(&left) == name)
    })
}

/// A class that takes properties it does not declare.
fn is_dynamic_class(cx: &Cx, class: &str) -> bool {
    let ty = Type::class(class);
    cx.index.ancestors(&ty).iter().any(|ancestor| {
        let decl = ancestor.class.decl;
        decl.name.eq_ignore_ascii_case("stdClass")
            || decl.name.eq_ignore_ascii_case("ArrayObject")
            || decl.name.eq_ignore_ascii_case("ArrayIterator")
            || decl
                .attributes
                .iter()
                .any(|attribute| attribute.name.eq_ignore_ascii_case("AllowDynamicProperties"))
    })
}

fn scoped_access(cx: &Cx, node: &SyntaxNode) {
    let (Some(qualifier), Some(name)) = (node.children().next(), member_name(node)) else {
        return;
    };
    let written = text_of(&name);
    if written == "class" {
        return;
    }
    if qualifier.kind() != NAME && unreliable_receiver(cx, node, &qualifier) {
        return;
    }
    let analyzer = cx.file.analyzer(node);
    let env = analyzer.env_around(node);
    let ty = analyzer.qualifier_type(&qualifier, &env);
    let Some((receiver, classes)) = classes_of(&analyzer, cx, &ty) else {
        return;
    };
    if is_callee(node) {
        scoped_call(
            cx, node, &qualifier, &name, &written, &receiver, &classes, &analyzer, &env,
        );
    } else {
        check_class_constant(cx, node, &name, &written, &receiver, &classes);
    }
}

#[allow(clippy::too_many_arguments)]
fn scoped_call(
    cx: &Cx,
    node: &SyntaxNode,
    qualifier: &SyntaxNode,
    name: &SyntaxNode,
    written: &str,
    receiver: &Type,
    classes: &[String],
    analyzer: &Analyzer<'_>,
    _env: &Env,
) {
    check_method(cx, node, name, written, receiver, classes, true, false);
    if !cx.on("static-call-of-instance-method") || qualifier.kind() != NAME {
        return;
    }
    let qualifier_text = text_of(qualifier);
    if is_relative_class(&qualifier_text.to_ascii_lowercase()) {
        return;
    }
    let found: Vec<_> = receiver
        .members()
        .iter()
        .filter_map(|member| cx.index.find_method(member, written))
        .collect();
    let Some(first) = found.first() else {
        return;
    };
    if found.iter().any(|found| found.member.is_static)
        || first.member.is_abstract && first.class.decl.kind != ClassKind::Interface
    {
        return;
    }
    if cx.has_magic(&classes[0], &["__callStatic"]) {
        return;
    }
    let target = &first.class.decl.name;
    let within_related_instance = has_this(node)
        && analyzer
            .class
            .as_ref()
            .is_some_and(|class| !class.anonymous && cx.index.is_subclass_of(&class.name, target))
        || has_this(node)
            && analyzer.class.as_ref().is_some_and(|class| {
                class.anonymous
                    && class
                        .parent
                        .as_ref()
                        .is_some_and(|parent| cx.index.is_subclass_of(parent, target))
            });
    if within_related_instance {
        return;
    }
    cx.report(
        "static-call-of-instance-method",
        name.text_range(),
        format!(
            "Non-static method '{}::{}' cannot be called statically",
            target, first.member.name
        ),
        Fix::None,
    );
}

/// Whether `$this` is there at a node: inside a method or closure that is not static.
fn has_this(node: &SyntaxNode) -> bool {
    for ancestor in node.ancestors() {
        match ancestor.kind() {
            METHOD_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR => {
                if super::util::has_modifier(&ancestor, STATIC_KW)
                    || (matches!(ancestor.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR)
                        && ast::tokens(&ancestor).any(|token| token.kind() == STATIC_KW))
                {
                    return false;
                }
                if ancestor.kind() == METHOD_DECLARATION {
                    return true;
                }
            }
            FUNCTION_DECLARATION => return false,
            _ => {}
        }
    }
    false
}

fn check_class_constant(
    cx: &Cx,
    node: &SyntaxNode,
    name: &SyntaxNode,
    written: &str,
    receiver: &Type,
    classes: &[String],
) {
    let found: Vec<_> = receiver
        .members()
        .iter()
        .filter_map(|member| cx.index.find_constant(member, written))
        .collect();
    if found.is_empty() {
        if !cx.on("undefined-class-constant")
            || guarded_by_check(node, &["defined(", "constant("], written)
            || classes
                .iter()
                .any(|class| a_subtype_declares_constant(cx, class, written))
        {
            return;
        }
        cx.report(
            "undefined-class-constant",
            name.text_range(),
            format!("Class constant '{written}' is not declared in {}", classes.join("|")),
            Fix::None,
        );
        return;
    }
    let first = &found[0];
    if cx.on("deprecated") && !inside_deprecated(node) {
        if let Some(reason) = deprecation(cx.index, first.member.doc.as_deref(), &first.member.attributes) {
            let label = format!("{}::{}", first.class.decl.name, first.member.name);
            cx.report(
                "deprecated",
                name.text_range(),
                deprecated_message("Constant", &label, &reason),
                Fix::None,
            );
        }
    }
}

fn static_property(cx: &Cx, node: &SyntaxNode) {
    let Some(qualifier) = node.children().next() else {
        return;
    };
    if is_callee(node) {
        return;
    }
    let Some(variable) = node
        .children()
        .filter(|child| child.kind() == VARIABLE_EXPR)
        .last()
        .filter(|variable| *variable != qualifier)
    else {
        return;
    };
    if reassigned_in_loop(&qualifier) {
        return;
    }
    let written = text_of(&variable).trim_start_matches('$').to_string();
    let analyzer = cx.file.analyzer(node);
    let env = analyzer.env_around(node);
    let ty = analyzer.qualifier_type(&qualifier, &env);
    let Some((receiver, classes)) = classes_of(&analyzer, cx, &ty) else {
        return;
    };
    let found: Vec<_> = receiver
        .members()
        .iter()
        .filter_map(|member| cx.index.find_property(member, &written))
        .collect();
    if found.is_empty() {
        if !cx.on("undefined-property") || guarded_by_check(node, &["property_exists("], &written) {
            return;
        }
        if classes.iter().any(|class| {
            !cx.is_user_class(class) || is_interface(cx, class) || a_subtype_declares(cx, class, &written, false)
        }) {
            return;
        }
        cx.report(
            "undefined-property",
            variable.text_range(),
            format!("Static property '${written}' is not declared in {}", classes.join("|")),
            Fix::None,
        );
        return;
    }
    let first = &found[0];
    if cx.on("deprecated") && !inside_deprecated(node) {
        if let Some(reason) = deprecation(cx.index, first.member.doc.as_deref(), &first.member.attributes) {
            let label = format!("{}::${}", first.class.decl.name, first.member.name);
            cx.report(
                "deprecated",
                variable.text_range(),
                deprecated_message("Property", &label, &reason),
                Fix::None,
            );
        }
    }
}

fn new_of_enum(cx: &Cx, node: &SyntaxNode) {
    if !cx.on("enum-misuse") {
        return;
    }
    let Some(name) = node.children().find(|child| child.kind() == NAME) else {
        return;
    };
    let written = text_of(&name);
    let analyzer = cx.file.analyzer(&name);
    let Type::Class { name: class, .. } = analyzer.class_type(&written) else {
        return;
    };
    let Some(found) = cx.index.class(&class) else {
        return;
    };
    if found.decl.kind == ClassKind::Enum {
        cx.report(
            "enum-misuse",
            name.text_range(),
            format!(
                "Enum '{}' cannot be instantiated, use one of its cases",
                found.decl.name
            ),
            Fix::None,
        );
    }
}

/// How a write to a readonly property is wrong, when it is.
fn check_readonly_write(
    cx: &Cx,
    access: &SyntaxNode,
    name: &SyntaxNode,
    declaring: &str,
    property: &str,
    promoted: bool,
    declared: Option<&Type>,
) {
    if is_element_write(access) && declared.and_then(kinds_of) != Some(Kinds::ARRAY) {
        // An object that takes element writes, such as `ArrayAccess`, changes itself.
        return;
    }
    let in_clone = access
        .ancestors()
        .filter(|ancestor| ancestor.kind() == METHOD_DECLARATION)
        .any(|method| ast::child_of(&method, NAME).is_some_and(|name| text_of(&name).eq_ignore_ascii_case("__clone")));
    if in_clone {
        return;
    }
    let analyzer = cx.file.analyzer(access);
    let plain = is_plain_assignment(access);
    let within_class = analyzer.class.as_ref().is_some_and(|class| {
        cx.index.is_subclass_of(&class.name, declaring) || class.name.eq_ignore_ascii_case(declaring)
    });
    let message = if !plain {
        format!("Cannot modify readonly property {declaring}::${property}")
    } else if !within_class {
        format!("Cannot initialize readonly property {declaring}::${property} from outside its class")
    } else if promoted && in_constructor(access) && is_this_access(access) {
        format!("Readonly property {declaring}::${property} is already initialized by its promoted parameter")
    } else {
        return;
    };
    cx.report("readonly-reassigned", name.text_range(), message, Fix::None);
}

fn is_element_write(access: &SyntaxNode) -> bool {
    access
        .parent()
        .is_some_and(|parent| parent.kind() == INDEX_EXPR && parent.children().next().as_ref() == Some(access))
}

fn is_this_access(access: &SyntaxNode) -> bool {
    access
        .children()
        .next()
        .is_some_and(|object| object.kind() == VARIABLE_EXPR && text_of(&object) == "$this")
}

fn in_constructor(node: &SyntaxNode) -> bool {
    node.ancestors()
        .find(|ancestor| ancestor.kind() == METHOD_DECLARATION)
        .and_then(|method| ast::child_of(&method, NAME))
        .is_some_and(|name| text_of(&name).eq_ignore_ascii_case("__construct"))
}

/// `$a->b = value`, as opposed to `$a->b .= value`, `$a->b++`, `$a->b[] = value` or `unset($a->b)`.
fn is_plain_assignment(access: &SyntaxNode) -> bool {
    let mut current = access.clone();
    while let Some(parent) = current.parent() {
        match parent.kind() {
            ARRAY_ITEM | ARRAY_EXPR | LIST_EXPR => current = parent,
            ASSIGN_EXPR => {
                return parent.children().next().as_ref() == Some(&current)
                    && ast::tokens(&parent).any(|token| token.kind() == ASSIGN);
            }
            _ => return false,
        }
    }
    false
}
