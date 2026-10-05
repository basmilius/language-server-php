//! Names for what a refactor creates, suggested from the expression and its type: the way a person
//! would name it, made unique among the names the scope already has.

use std::collections::HashSet;

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode};

use super::exprs::name_of;
use crate::ast::text_of;
use crate::infer::literal_string;
use crate::inspections::Cx;

/// `snake_case`, `kebab-case`, `PascalCase` and `UPPER_CASE` as `camelCase`.
pub(crate) fn camel(text: &str) -> String {
    let parts: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect();
    let mut out = String::new();
    for (position, part) in parts.iter().enumerate() {
        let shout = part.len() > 1 && part.chars().all(|c| !c.is_lowercase());
        let word: String = if shout {
            part.to_lowercase()
        } else {
            (*part).to_string()
        };
        let mut characters = word.chars();
        let Some(first) = characters.next() else {
            continue;
        };
        if position == 0 {
            out.extend(first.to_lowercase());
        } else {
            out.extend(first.to_uppercase());
        }
        out.push_str(characters.as_str());
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert_str(0, "value");
    }
    out
}

/// `UPPER_SNAKE_CASE` from any spelling.
pub(crate) fn upper_snake(text: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && previous_lower {
                out.push('_');
            }
            out.extend(c.to_uppercase());
            previous_lower = c.is_lowercase() || c.is_ascii_digit();
        } else {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            previous_lower = false;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("VALUE_{out}")
    } else {
        out
    }
}

const VERBS: &[&str] = &[
    "get",
    "find",
    "load",
    "fetch",
    "create",
    "make",
    "build",
    "read",
    "calculate",
    "compute",
    "resolve",
];

/// The noun of a method or function name: `getUserName` is `userName`.
fn noun_of(name: &str) -> String {
    let camelled = camel(name);
    for verb in VERBS {
        if let Some(rest) = camelled.strip_prefix(verb) {
            if rest.chars().next().is_some_and(char::is_uppercase) {
                return camel(rest);
            }
        }
    }
    camelled
}

fn from_type(ty: &Type) -> Option<String> {
    let ty = ty.without_null();
    Some(match &ty {
        Type::Int | Type::IntLiteral(_) => "int".to_string(),
        Type::Float => "float".to_string(),
        Type::String | Type::StringLiteral(_) => "string".to_string(),
        Type::Bool | Type::True | Type::False => "bool".to_string(),
        Type::Array(..) | Type::List(_) | Type::Shape(_) => "array".to_string(),
        Type::Callable(_) => "callback".to_string(),
        Type::Class { name, .. } => camel(crate::short(name)),
        _ => return None,
    })
}

fn member_name(access: &SyntaxNode) -> Option<String> {
    access
        .children()
        .filter(|node| node.kind() == NAME)
        .last()
        .map(|name| text_of(&name))
}

/// What the expression says about its own name, before its type is asked.
fn from_expression(expr: &SyntaxNode) -> Option<String> {
    match expr.kind() {
        CALL_EXPR => {
            let callee = expr.children().next()?;
            match callee.kind() {
                NAME => Some(noun_of(&text_of(&callee))),
                PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR => member_name(&callee).map(|name| noun_of(&name)),
                _ => None,
            }
        }
        NEW_EXPR => expr
            .children()
            .find(|child| child.kind() == NAME)
            .map(|name| camel(crate::short(&text_of(&name)))),
        PROPERTY_FETCH_EXPR => member_name(expr).map(|name| camel(&name)),
        STATIC_PROPERTY_EXPR => expr
            .children()
            .nth(1)
            .map(|name| camel(text_of(&name).trim_start_matches('$'))),
        SCOPED_ACCESS_EXPR => {
            let member = member_name(expr)?;
            if member.eq_ignore_ascii_case("class") {
                Some("className".to_string())
            } else {
                Some(camel(&member))
            }
        }
        INDEX_EXPR => {
            let key = expr.children().nth(1)?;
            let text = literal_string(&key)?;
            (text.len() <= 24).then(|| camel(&text))
        }
        NAME => Some(camel(&text_of(expr))),
        LITERAL => {
            let text = literal_string(expr)?;
            let words = text
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .count();
            let simple = text
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'));
            (simple && words > 0 && words <= 3 && text.len() <= 24).then(|| camel(&text))
        }
        PAREN_EXPR => expr.children().next().and_then(|inner| from_expression(&inner)),
        _ => None,
    }
}

/// A name for the value of an expression.
pub(crate) fn variable_name(cx: &Cx, expr: &SyntaxNode) -> String {
    let mut name = from_expression(expr).filter(|name| usable(name));
    if name.is_none() {
        let analyzer = cx.file.analyzer(expr);
        let env = analyzer.env_around(expr);
        name = from_type(&analyzer.type_of(expr, &env));
    }
    name.filter(|name| usable(name)).unwrap_or_else(|| "value".to_string())
}

fn usable(name: &str) -> bool {
    !name.is_empty()
        && name != "this"
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !name.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// The variable names a scope uses, parameters and `$this` included.
pub(crate) fn taken_variables(scope: &SyntaxNode) -> HashSet<String> {
    let mut taken: HashSet<String> = ["this".to_string()].into();
    for element in scope.descendants_with_tokens() {
        if let SyntaxElement::Token(token) = element {
            if token.kind() == VARIABLE {
                taken.insert(token.text().trim_start_matches('$').to_string());
            }
        }
    }
    taken
}

pub(crate) fn unique(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(name) {
        return name.to_string();
    }
    (2..)
        .map(|suffix| format!("{name}{suffix}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| name.to_string())
}

/// A name for a constant that holds the value of an expression.
pub(crate) fn constant_name(expr: &SyntaxNode) -> String {
    let named = match expr.kind() {
        LITERAL => literal_string(expr)
            .filter(|text| text.len() <= 24 && text.chars().any(char::is_alphabetic))
            .map(|text| upper_snake(&text)),
        _ => name_of(expr).map(|name| upper_snake(&name)),
    };
    named
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| match expr.kind() {
            ARRAY_EXPR => "ITEMS".to_string(),
            _ => "VALUE".to_string(),
        })
}

/// The fully qualified name a written name stands for where the resolver reads it, with the
/// leading backslash, or `None` for what is not a class, function or constant name.
pub(crate) fn fully_qualified(
    index: &php_index::Index,
    resolver: &php_index::NameResolver,
    name: &SyntaxNode,
) -> Option<String> {
    let parent = name.parent()?;
    let written = text_of(name);
    if written.starts_with('\\')
        || matches!(
            written.to_ascii_lowercase().as_str(),
            "true"
                | "false"
                | "null"
                | "self"
                | "static"
                | "parent"
                | "int"
                | "float"
                | "string"
                | "bool"
                | "array"
                | "callable"
                | "iterable"
                | "object"
                | "mixed"
                | "void"
                | "never"
                | "numeric"
        )
    {
        return None;
    }
    let first = parent.children().next().as_ref() == Some(name);
    let resolved = match parent.kind() {
        NEW_EXPR | NAMED_TYPE | CATCH_CLAUSE | ATTRIBUTE | EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE => {
            resolver.resolve_class(&written)
        }
        SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR if first => resolver.resolve_class(&written),
        BINARY_EXPR if !first => resolver.resolve_class(&written),
        CALL_EXPR if first => {
            let candidates = resolver.function_candidates(&written);
            candidates
                .iter()
                .find(|candidate| index.function(candidate).is_some())
                .or(candidates.last())
                .cloned()?
        }
        _ if super::exprs::is_expression(name) => {
            let candidates = resolver.constant_candidates(&written);
            candidates
                .iter()
                .find(|candidate| index.constant(candidate).is_some())
                .or(candidates.last())
                .cloned()?
        }
        _ => return None,
    };
    Some(format!("\\{}", resolved.trim_start_matches('\\')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_cases_every_spelling() {
        assert_eq!(camel("user_name"), "userName");
        assert_eq!(camel("UserName"), "userName");
        assert_eq!(camel("USER_NAME"), "userName");
        assert_eq!(camel("foo-bar baz"), "fooBarBaz");
        assert_eq!(camel("2fa"), "value2fa");
    }

    #[test]
    fn upper_snake_cases_every_spelling() {
        assert_eq!(upper_snake("userName"), "USER_NAME");
        assert_eq!(upper_snake("hello world"), "HELLO_WORLD");
        assert_eq!(upper_snake("already_snake"), "ALREADY_SNAKE");
    }

    #[test]
    fn a_verb_in_front_of_a_noun_is_dropped() {
        assert_eq!(noun_of("getUserName"), "userName");
        assert_eq!(noun_of("get"), "get");
        assert_eq!(noun_of("count"), "count");
        assert_eq!(noun_of("array_map"), "arrayMap");
    }

    #[test]
    fn a_taken_name_gets_a_number() {
        let taken: HashSet<String> = ["a".to_string(), "a2".to_string()].into();
        assert_eq!(unique("a", &taken), "a3");
        assert_eq!(unique("b", &taken), "b");
    }
}
