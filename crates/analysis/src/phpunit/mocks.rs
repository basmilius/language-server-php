//! Test doubles: which class a mock stands for, and the strings that name its methods.

use php_index::test_facts::{class_constant, string_value};
use php_index::{Name, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::strings::{Role, StringTarget, TestString};
use crate::ast::range_of;
use crate::infer::{Analyzer, Env, arguments};

/// The namespace of PHPUnit's test double classes.
const MOCK_OBJECT: &str = "PHPUnit\\Framework\\MockObject\\";

/// The class a double of this type stands for: the member of `MockObject&Foo` that is not the double
/// itself, or the `Foo` of a `MockBuilder<Foo>`.
fn mocked_in(ty: &Type) -> Option<Name> {
    for member in ty.members() {
        match member {
            Type::Intersection(parts) => {
                let names: Vec<&str> = parts
                    .iter()
                    .filter_map(|part| match part {
                        Type::Class { name, .. } => Some(name.as_str()),
                        _ => None,
                    })
                    .collect();
                if names.iter().any(|name| name.starts_with(MOCK_OBJECT)) {
                    if let Some(name) = names.iter().find(|name| !name.starts_with(MOCK_OBJECT)) {
                        return Some((*name).to_string());
                    }
                }
            }
            Type::Class { name, args } if name.starts_with(MOCK_OBJECT) => {
                if let Some(Type::Class { name, .. }) = args.first() {
                    return Some(name.clone());
                }
            }
            _ => {}
        }
    }
    None
}

/// The class a test double expression stands for. A call on a double (`$mock->expects(..)`,
/// `$mock->method('a')`) stands for the same one as the double it was called on.
pub fn mocked_class(analyzer: &Analyzer<'_>, expression: &SyntaxNode, env: &Env) -> Option<Name> {
    let mut current = expression.clone();
    for _ in 0..8 {
        let ty = analyzer.type_of(&current, env);
        if let Some(class) = mocked_in(&ty) {
            return Some(class);
        }
        current = match current.kind() {
            CALL_EXPR => {
                let callee = current.children().next()?;
                if callee.kind() != PROPERTY_FETCH_EXPR {
                    return None;
                }
                callee.children().next()?
            }
            PAREN_EXPR => current.children().next()?,
            _ => return None,
        };
    }
    None
}

/// The strings of a call that name methods of a double: `->method('a')`, `->onlyMethods(['a'])`
/// and `createPartialMock(Foo::class, ['a'])`.
pub fn call_strings(analyzer: &Analyzer<'_>, call: &SyntaxNode) -> Vec<TestString> {
    let Some(callee) = call
        .children()
        .next()
        .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
    else {
        return Vec::new();
    };
    let Some(name) = callee.children().find(|child| child.kind() == NAME) else {
        return Vec::new();
    };
    let written = name.text().to_string().to_ascii_lowercase();
    let args = arguments(call);
    let (class, candidates) = match written.as_str() {
        "method" => {
            let Some(object) = callee.children().next() else {
                return Vec::new();
            };
            let env = analyzer.env_around(call);
            (
                mocked_class(analyzer, &object, &env),
                args.first().and_then(|arg| arg.expr.clone()).into_iter().collect(),
            )
        }
        "onlymethods" | "addmethods" => {
            let Some(object) = callee.children().next() else {
                return Vec::new();
            };
            let env = analyzer.env_around(call);
            (
                mocked_class(analyzer, &object, &env),
                args.first()
                    .and_then(|arg| arg.expr.as_ref())
                    .map_or_else(Vec::new, list_items),
            )
        }
        "createpartialmock" => (
            args.first()
                .and_then(|arg| arg.expr.as_ref())
                .and_then(|expr| class_constant(expr, &analyzer.resolver)),
            args.get(1)
                .and_then(|arg| arg.expr.as_ref())
                .map_or_else(Vec::new, list_items),
        ),
        _ => return Vec::new(),
    };
    let Some(class) = class else {
        return Vec::new();
    };
    candidates
        .iter()
        .filter_map(string_value)
        .map(|(value, span)| TestString {
            range: range_of(span.start, span.end),
            value,
            target: StringTarget::Method {
                class: class.clone(),
                role: Role::Mocked,
            },
            declaration: false,
        })
        .collect()
}

/// The items of an array written out.
fn list_items(list: &SyntaxNode) -> Vec<SyntaxNode> {
    if list.kind() != ARRAY_EXPR {
        return Vec::new();
    }
    list.children()
        .filter(|item| item.kind() == ARRAY_ITEM)
        .filter_map(|item| item.children().last())
        .collect()
}
