//! Names of classes, functions and constants: the ones that do not exist, and the ones that are
//! deprecated.

use php_index::{Attribute, Doc, Index, UseKind};
use php_syntax::PhpVersion;
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::util::{guarded_by_check, is_builtin_type, is_relative_class, member_name};
use super::{Cx, Fix};
use crate::ast::{self, start, text_of};

/// What a name node stands for.
enum Role {
    /// A class. `undefined` says whether PHP minds one that does not exist here.
    Class {
        undefined: bool,
    },
    Constant,
    Other,
}

pub(super) fn run(cx: &Cx) {
    for node in &cx.nodes {
        match node.kind() {
            NAME => check_name(cx, node),
            CALL_EXPR => check_function_call(cx, node),
            _ => {}
        }
    }
}

fn role(name: &SyntaxNode) -> Role {
    let Some(parent) = name.parent() else {
        return Role::Other;
    };
    let first = parent.children().next().as_ref() == Some(name);
    match parent.kind() {
        NAMED_TYPE => Role::Class {
            undefined: !in_catch(&parent),
        },
        NEW_EXPR | TRAIT_USE => Role::Class { undefined: true },
        // PHP only loads an attribute when it is read, but a framework that reads it silently skips
        // a missing one, which turns a guard such as `#[IsGranted]` off.
        ATTRIBUTE => Role::Class { undefined: true },
        SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR if first => {
            let is_class_constant = member_name(&parent).is_some_and(|member| text_of(&member) == "class");
            Role::Class {
                undefined: !is_class_constant,
            }
        }
        BINARY_EXPR => {
            let is_instanceof = ast::tokens(&parent).any(|token| token.kind() == INSTANCEOF_KW);
            if is_instanceof && !first {
                Role::Class { undefined: false }
            } else {
                Role::Constant
            }
        }
        CALL_EXPR if !first => Role::Constant,
        CONST_ELEMENT | ENUM_CASE | DECLARE_DIRECTIVE if !first => Role::Constant,
        ARGUMENT | ARRAY_ITEM | ASSIGN_EXPR | TERNARY_EXPR | PAREN_EXPR | RETURN_STATEMENT | ECHO_STATEMENT
        | EXPR_STATEMENT | MATCH_ARM | MATCH_EXPR | INDEX_EXPR | PREFIX_EXPR | CAST_EXPR | CASE_CLAUSE
        | IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT
        | SWITCH_STATEMENT | PARAMETER | PROPERTY_ELEMENT | STATIC_VARIABLE | THROW_EXPR | YIELD_EXPR | CLONE_EXPR
        | PRINT_EXPR => Role::Constant,
        _ => Role::Other,
    }
}

/// A class in a `catch`, which may well be one that is not installed.
fn in_catch(named_type: &SyntaxNode) -> bool {
    let mut current = named_type.clone();
    while let Some(parent) = current.parent() {
        match parent.kind() {
            UNION_TYPE => current = parent,
            CATCH_CLAUSE => return true,
            _ => return false,
        }
    }
    false
}

fn check_name(cx: &Cx, node: &SyntaxNode) {
    match role(node) {
        Role::Class { undefined } => check_class(cx, node, undefined),
        Role::Constant => check_constant(cx, node),
        Role::Other => {}
    }
}

fn check_class(cx: &Cx, node: &SyntaxNode, report_undefined: bool) {
    let written = text_of(node);
    let lower = written.to_ascii_lowercase();
    if is_builtin_type(&lower) || is_relative_class(&lower) {
        return;
    }
    let analyzer = cx.file.analyzer(node);
    let resolved = analyzer.resolver.resolve_class(&written);
    let Some(class) = cx.index.class(&resolved) else {
        if report_undefined
            && cx.ready
            && cx.on("undefined-class")
            && !class_may_exist(cx, node, &resolved)
            && !is_editor_attribute(&resolved)
        {
            let fix = if written.contains('\\') {
                Fix::None
            } else {
                Fix::Import {
                    name: written.clone(),
                    kind: UseKind::Class,
                    at: start(node),
                }
            };
            cx.report(
                "undefined-class",
                node.text_range(),
                format!("Undefined class '{}'", written.trim_start_matches('\\')),
                fix,
            );
        }
        return;
    };
    if cx.on("deprecated") && !inside_deprecated(node) {
        if let Some(reason) = deprecation(cx.index, class.decl.doc.as_deref(), &class.decl.attributes) {
            cx.report(
                "deprecated",
                node.text_range(),
                deprecated_message("Class", &class.decl.name, &reason),
                Fix::None,
            );
        }
    }
}

/// PhpStorm's attributes are hints for the editor that nothing installs or reads, and the stubs keep
/// them in `meta/`, which the index skips.
fn is_editor_attribute(resolved: &str) -> bool {
    resolved
        .get(..19)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("JetBrains\\PhpStorm\\"))
}

/// A class the index does not hold may still exist: Composer can load it, an extension the project
/// does not list declares it, or the code only runs when it does.
fn class_may_exist(cx: &Cx, node: &SyntaxNode, resolved: &str) -> bool {
    (cx.externals.class)(resolved)
        || guarded_by_check(
            node,
            &["class_exists(", "interface_exists(", "trait_exists(", "enum_exists("],
            crate::short(resolved),
        )
        || cx.text.contains("class_alias(")
}

fn check_constant(cx: &Cx, node: &SyntaxNode) {
    let written = text_of(node);
    if matches!(written.to_ascii_lowercase().as_str(), "true" | "false" | "null") {
        return;
    }
    let analyzer = cx.file.analyzer(node);
    let candidates = analyzer.resolver.constant_candidates(&written);
    let Some(constant) = cx.index.first_constant(&candidates) else {
        if cx.ready
            && cx.on("undefined-constant")
            && !cx.has_dynamic_defines()
            && !analyzer
                .resolver
                .imports(UseKind::Constant)
                .any(|(alias, _)| alias == written)
            && !candidates.iter().any(|candidate| (cx.externals.constant)(candidate))
            && !guarded_by_check(node, &["defined("], &written)
        {
            let fix = if written.contains('\\') {
                Fix::None
            } else {
                Fix::Import {
                    name: written.clone(),
                    kind: UseKind::Constant,
                    at: start(node),
                }
            };
            cx.report(
                "undefined-constant",
                node.text_range(),
                format!("Undefined constant '{}'", written.trim_start_matches('\\')),
                fix,
            );
        }
        return;
    };
    if cx.on("deprecated") && !inside_deprecated(node) {
        if let Some(reason) = deprecation(cx.index, constant.decl.doc.as_deref(), &[]) {
            cx.report(
                "deprecated",
                node.text_range(),
                deprecated_message("Constant", &constant.decl.name, &reason),
                Fix::None,
            );
        }
    }
}

fn check_function_call(cx: &Cx, call: &SyntaxNode) {
    let Some(callee) = call.children().next().filter(|callee| callee.kind() == NAME) else {
        return;
    };
    let written = text_of(&callee);
    let analyzer = cx.file.analyzer(&callee);
    let candidates = analyzer.resolver.function_candidates(&written);
    let Some(function) = cx.index.first_function(&candidates) else {
        if cx.ready
            && cx.on("undefined-function")
            && !analyzer
                .resolver
                .imports(UseKind::Function)
                .any(|(alias, _)| alias.eq_ignore_ascii_case(&written))
            && !candidates.iter().any(|candidate| (cx.externals.function)(candidate))
            && !guarded_by_check(&callee, &["function_exists("], &written)
        {
            let fix = if written.contains('\\') {
                Fix::None
            } else {
                Fix::Import {
                    name: written.clone(),
                    kind: UseKind::Function,
                    at: start(&callee),
                }
            };
            cx.report(
                "undefined-function",
                callee.text_range(),
                format!("Undefined function '{}'", written.trim_start_matches('\\')),
                fix,
            );
        }
        return;
    };
    if cx.on("deprecated") && !inside_deprecated(call) {
        let level = cx.index.level;
        let doc = function.decl.doc.as_deref();
        let attributes = &function.decl.attributes;
        if let Some(reason) = deprecation_at(level, doc, attributes) {
            cx.report(
                "deprecated",
                callee.text_range(),
                deprecated_message("Function", &function.decl.name, &reason),
                Fix::None,
            );
        }
    }
}

pub(super) fn deprecated_message(kind: &str, name: &str, reason: &str) -> String {
    if reason.is_empty() {
        format!("{kind} '{name}' is deprecated")
    } else {
        format!("{kind} '{name}' is deprecated: {reason}")
    }
}

/// The explanation of a deprecation that holds at the index's level, empty when there is none.
pub(super) fn deprecation(index: &Index, doc: Option<&Doc>, attributes: &[Attribute]) -> Option<String> {
    deprecation_at(index.level, doc, attributes)
}

fn deprecation_at(level: PhpVersion, doc: Option<&Doc>, attributes: &[Attribute]) -> Option<String> {
    let deprecated = doc?.deprecated.clone()?;
    let attribute = attributes
        .iter()
        .find(|attribute| attribute.name.ends_with("\\Deprecated") || attribute.name == "Deprecated");
    if let Some(attribute) = attribute {
        let since = attribute
            .args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("since"))
            .and_then(|arg| PhpVersion::parse(arg.value.trim_matches(['\'', '"'])));
        if since.is_some_and(|since| level < since) {
            return None;
        }
        let reason = attribute
            .args
            .iter()
            .find(|arg| matches!(arg.name.as_deref(), None | Some("reason")))
            .map(|arg| arg.value.trim_matches(['\'', '"']).to_string())
            .unwrap_or_default();
        return Some(reason);
    }
    Some(first_line(&deprecated))
}

fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// Using something deprecated from inside something deprecated is how it gets retired.
pub(super) fn inside_deprecated(node: &SyntaxNode) -> bool {
    node.ancestors().any(|ancestor| {
        if !(ast::is_class_like(ancestor.kind()) || ast::is_function_like(ancestor.kind())) {
            return false;
        }
        ancestor
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .any(|token| token.kind() == DOC_COMMENT && token.text().contains("@deprecated"))
    })
}
