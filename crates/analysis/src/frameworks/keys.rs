//! Strings that name something a project declares: `config('app.name')`, `route('home')`,
//! `view('mail.welcome')`. Which arguments of which functions do is the framework overlay's; what
//! the strings can say and where they point is the catalogs of `php-index`.

use php_index::framework::keys::KeyKind;
use php_index::framework::overlay::{Marker, is_marked, markers_for};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use crate::ast::range_of;
use crate::context::FileContext;
use crate::infer::Analyzer;

/// A string that names something, with the text inside its quotes.
#[derive(Clone, Debug, PartialEq)]
pub struct KeyString {
    pub kind: KeyKind,
    pub value: String,
    pub range: TextRange,
    /// The class the call is made on, for the kinds that depend on it.
    pub scope: Option<String>,
    /// The code expects the name to be missing: it asks whether it exists or gives a default.
    pub guarded: bool,
}

/// What a call, a `new` or an attribute is, as far as the arguments it takes are concerned.
struct Callee {
    declaring: Option<String>,
    receiver: Option<String>,
    receiver_type: Option<php_index::Type>,
    method: String,
    params: Vec<String>,
}

/// Where a literal stands in the argument of a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// It is the argument.
    Argument,
    /// It is the key of an item of the array the argument is.
    Key,
    /// It is an item of the list the argument is.
    Item,
}

/// The argument a literal is in, and where in it.
fn argument_of(literal: &SyntaxNode) -> Option<(SyntaxNode, Place)> {
    let parent = literal.parent()?;
    match parent.kind() {
        ARGUMENT => Some((parent, Place::Argument)),
        ARRAY_ITEM => {
            let children: Vec<SyntaxNode> = parent.children().collect();
            let place = match children.as_slice() {
                [key, _] if key == literal => Place::Key,
                [only] if only == literal => Place::Item,
                _ => return None,
            };
            let array = parent.parent().filter(|node| node.kind() == ARRAY_EXPR)?;
            let argument = array.parent().filter(|node| node.kind() == ARGUMENT)?;
            Some((argument, place))
        }
        _ => None,
    }
}

fn callees_of(analyzer: &Analyzer<'_>, owner: &SyntaxNode) -> Vec<Callee> {
    let level = analyzer.index.level;
    match owner.kind() {
        CALL_EXPR => {
            let Some(callee) = owner.children().next() else {
                return Vec::new();
            };
            let Some(name) = callee.descendants().filter(|node| node.kind() == NAME).last() else {
                return Vec::new();
            };
            if !is_marked(&name.text().to_string()) {
                return Vec::new();
            }
            let env = analyzer.env_around(owner);
            analyzer
                .callees(owner, &env)
                .into_iter()
                .map(|resolved| {
                    let (declaring, method) = match resolved.name.split_once("::") {
                        Some((class, method)) => (Some(class.to_string()), method.to_string()),
                        None => (None, resolved.name.clone()),
                    };
                    Callee {
                        declaring,
                        receiver: resolved
                            .receiver
                            .as_ref()
                            .and_then(|ty| ty.class_names().first().map(|name| name.to_string())),
                        receiver_type: resolved.receiver.clone(),
                        method,
                        params: resolved
                            .callable
                            .params_at(level)
                            .map(|param| param.name.clone())
                            .collect(),
                    }
                })
                .collect()
        }
        NEW_EXPR | ATTRIBUTE => {
            if !is_marked("__construct") {
                return Vec::new();
            }
            let Some(name) = owner.children().find(|child| child.kind() == NAME) else {
                return Vec::new();
            };
            let class = analyzer.resolver.resolve_class(&name.text().to_string());
            let params = analyzer
                .index
                .find_declared_method(&php_index::Type::class(class.clone()), "__construct")
                .map(|found| {
                    found
                        .member
                        .callable
                        .params_at(level)
                        .map(|param| param.name.clone())
                        .collect()
                })
                .unwrap_or_default();
            vec![Callee {
                declaring: Some(class.clone()),
                receiver_type: Some(php_index::Type::class(class.clone())),
                receiver: Some(class),
                method: "__construct".to_string(),
                params,
            }]
        }
        _ => Vec::new(),
    }
}

/// The key a string literal is, when it is the argument of a call, a `new` or an attribute that takes
/// one. The offset says which `%parameter%` of an expression is meant.
pub fn key_of_literal(analyzer: &Analyzer<'_>, literal: &SyntaxNode, at: Option<u32>) -> Option<KeyString> {
    if !analyzer.index.frameworks().any() {
        return None;
    }
    let (value, span) = php_index::test_facts::string_value(literal)?;
    let Some((argument, place)) = argument_of(literal) else {
        return event_key(analyzer, literal, &value, span)
            .or_else(|| super::casts::cast_key(analyzer, literal, &value, span));
    };
    let named = argument
        .children_with_tokens()
        .any(|element| element.kind() == COLON)
        .then(|| {
            argument
                .children_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| !token.kind().is_trivia())
                .map(|token| token.text().to_string())
        })
        .flatten();
    let list = argument.parent().filter(|list| list.kind() == ARGUMENT_LIST)?;
    let owner = list
        .parent()
        .filter(|owner| matches!(owner.kind(), CALL_EXPR | NEW_EXPR | ATTRIBUTE))?;
    let position = list
        .children()
        .filter(|child| child.kind() == ARGUMENT)
        .position(|child| child == argument)?;
    let argument_count = list.children().filter(|child| child.kind() == ARGUMENT).count();
    for callee in callees_of(analyzer, &owner) {
        let index_of = match &named {
            Some(name) => callee.params.iter().position(|param| param == name),
            None => Some(position),
        };
        let Some(index_of) = index_of else {
            continue;
        };
        let param_name = callee.params.get(index_of);
        let markers = markers_for(
            analyzer.index,
            callee.declaring.as_deref(),
            callee.receiver.as_deref(),
            &callee.method,
        );
        for marker in markers {
            let Marker::Key {
                kind,
                position: wanted,
                name,
            } = marker
            else {
                continue;
            };
            let matches = match &name {
                Some(name) => param_name == Some(name),
                None => wanted == index_of,
            };
            if !matches {
                continue;
            }
            let range = range_of(span.start, span.end);
            if kind == "expression" {
                return placeholder_at(&value, span.start, at?);
            }
            let Some(kind) = KeyKind::parse(&kind) else {
                continue;
            };
            let fits = match kind {
                KeyKind::EntityField => place == Place::Key,
                KeyKind::Relation => true,
                _ => place == Place::Argument,
            };
            if !fits {
                continue;
            }
            let asks_existence = matches!(callee.method.to_ascii_lowercase().as_str(), "has" | "exists");
            let has_default = kind == KeyKind::Config && argument_count > 1;
            let scope = match kind {
                KeyKind::EntityField => callee.receiver.as_ref().and_then(|receiver| {
                    php_index::framework::symfony::doctrine::entity_of_repository(
                        analyzer.index,
                        &php_index::Type::class(receiver.clone()),
                    )
                }),
                KeyKind::Relation => callee
                    .receiver_type
                    .as_ref()
                    .and_then(|receiver| super::relations::model_of(analyzer, receiver)),
                _ => callee.receiver.clone(),
            };
            return Some(KeyString {
                kind,
                value,
                range,
                scope,
                guarded: asks_existence || has_default,
            });
        }
    }
    None
}

/// Whether an array is the argument a callee reads as validation rules by field.
pub(crate) fn is_rules_argument(analyzer: &Analyzer<'_>, array: &SyntaxNode) -> bool {
    let Some(argument) = array.parent().filter(|node| node.kind() == ARGUMENT) else {
        return false;
    };
    let Some(list) = argument.parent().filter(|node| node.kind() == ARGUMENT_LIST) else {
        return false;
    };
    let Some(owner) = list
        .parent()
        .filter(|owner| matches!(owner.kind(), CALL_EXPR | NEW_EXPR))
    else {
        return false;
    };
    let Some(position) = list
        .children()
        .filter(|child| child.kind() == ARGUMENT)
        .position(|child| child == argument)
    else {
        return false;
    };
    let named = argument
        .children_with_tokens()
        .any(|element| element.kind() == COLON)
        .then(|| {
            argument
                .children_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| !token.kind().is_trivia())
                .map(|token| token.text().to_string())
        })
        .flatten();
    callees_of(analyzer, &owner).into_iter().any(|callee| {
        let index_of = match &named {
            Some(name) => callee.params.iter().position(|param| param == name),
            None => Some(position),
        };
        markers_for(
            analyzer.index,
            callee.declaring.as_deref(),
            callee.receiver.as_deref(),
            &callee.method,
        )
        .into_iter()
        .any(|marker| matches!(marker, Marker::Rules { position: wanted } if Some(wanted) == index_of))
    })
}

/// A key of the array a subscriber returns, which names the event it listens to.
fn event_key(analyzer: &Analyzer<'_>, literal: &SyntaxNode, value: &str, span: php_index::Span) -> Option<KeyString> {
    let item = literal.parent().filter(|node| node.kind() == ARRAY_ITEM)?;
    if item.children().next().as_ref() != Some(literal) || item.children().count() != 2 {
        return None;
    }
    let array = item.parent().filter(|node| node.kind() == ARRAY_EXPR)?;
    array.parent().filter(|node| node.kind() == RETURN_STATEMENT)?;
    let method = array.ancestors().find(|node| node.kind() == METHOD_DECLARATION)?;
    let name = method.children().find(|child| child.kind() == NAME)?.text().to_string();
    if !is_marked(&name) {
        return None;
    }
    let class = analyzer.class.as_ref()?.name.clone();
    markers_for(analyzer.index, Some(&class), Some(&class), &name)
        .into_iter()
        .find_map(|marker| match marker {
            Marker::ReturnKeys { kind } => KeyKind::parse(&kind),
            _ => None,
        })
        .map(|kind| KeyString {
            kind,
            value: value.to_string(),
            range: range_of(span.start, span.end),
            scope: None,
            guarded: true,
        })
}

/// The classes an argument of the call around a token may be, when the callee says so.
pub fn class_argument_base(analyzer: &Analyzer<'_>, token: &php_syntax::SyntaxToken) -> Option<String> {
    if !analyzer.index.frameworks().any() {
        return None;
    }
    let argument = token.parent_ancestors().find(|node| node.kind() == ARGUMENT)?;
    let list = argument.parent().filter(|list| list.kind() == ARGUMENT_LIST)?;
    let owner = list.parent().filter(|owner| owner.kind() == CALL_EXPR)?;
    let position = list
        .children()
        .filter(|child| child.kind() == ARGUMENT)
        .position(|child| child == argument)?;
    for callee in callees_of(analyzer, &owner) {
        for marker in markers_for(
            analyzer.index,
            callee.declaring.as_deref(),
            callee.receiver.as_deref(),
            &callee.method,
        ) {
            if let Marker::ClassArgument { base, position: wanted } = marker {
                if wanted == position {
                    return Some(base);
                }
            }
        }
    }
    None
}

/// The `%name%` or `%env(NAME)%` of a string that the offset is in.
fn placeholder_at(value: &str, start: u32, at: u32) -> Option<KeyString> {
    let mut from = 0;
    while let Some(open) = value[from..].find('%') {
        let begin = from + open;
        if value[begin + 1..].starts_with('%') {
            from = begin + 2;
            continue;
        }
        let Some(length) = value[begin + 1..].find('%') else {
            break;
        };
        let inner_start = begin + 1;
        let inner = &value[inner_start..inner_start + length];
        let end = inner_start + length;
        from = end + 1;
        let (kind, name_start, name) = match inner.strip_prefix("env(").and_then(|rest| rest.strip_suffix(')')) {
            Some(env) => {
                let skipped = env.rfind(':').map_or(0, |colon| colon + 1);
                (KeyKind::Env, inner_start + 4 + skipped, &env[skipped..])
            }
            None => (KeyKind::Parameter, inner_start, inner),
        };
        let (name_from, name_to) = (start + name_start as u32, start + (name_start + name.len()) as u32);
        if name_from <= at && at <= name_to + 1 {
            return Some(KeyString {
                kind,
                value: name.to_string(),
                range: range_of(name_from, name_to),
                scope: None,
                guarded: true,
            });
        }
    }
    None
}

/// The key string around an offset, the end of the text inside the quotes counting as in it.
pub fn key_at(analyzer: &Analyzer<'_>, offset: u32) -> Option<KeyString> {
    let tokens = match analyzer.root.token_at_offset(php_syntax::TextSize::from(offset)) {
        php_syntax::TokenAtOffset::None => return None,
        php_syntax::TokenAtOffset::Single(token) => vec![token],
        php_syntax::TokenAtOffset::Between(left, right) => vec![right, left],
    };
    for token in tokens {
        if token.kind() != STRING_LITERAL {
            continue;
        }
        let Some(literal) = token.parent() else {
            continue;
        };
        if let Some(found) = key_of_literal(analyzer, &literal, Some(offset)) {
            let (start, end) = (u32::from(found.range.start()), u32::from(found.range.end()));
            if start <= offset && offset <= end {
                return Some(found);
            }
        }
    }
    None
}

/// Every key string of a file.
pub fn keys_in(ctx: &FileContext<'_>) -> Vec<KeyString> {
    if !ctx.index.frameworks().any() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for node in ctx.root.descendants().filter(|node| node.kind() == LITERAL) {
        if argument_of(&node).is_none() && node.parent().is_none_or(|parent| parent.kind() != ARRAY_ITEM) {
            continue;
        }
        let analyzer = ctx.analyzer(&node);
        out.extend(key_of_literal(&analyzer, &node, None));
    }
    out
}

/// What a key stands for, one description per place it is declared.
pub fn describe(
    index: &php_index::Index,
    kind: KeyKind,
    name: &str,
    scope: Option<&str>,
) -> Vec<crate::nav::Description> {
    let definitions = php_index::framework::keys::definitions(index, kind, name, scope);
    let title = format!("{} '{name}'", kind.label());
    if definitions.is_empty() {
        return vec![crate::nav::Description {
            title,
            signature: name.to_string(),
            doc: None,
            place: None,
        }];
    }
    definitions
        .into_iter()
        .map(|definition| crate::nav::Description {
            title: title.clone(),
            signature: if definition.detail.is_empty() {
                name.to_string()
            } else {
                format!("{name} = {}", definition.detail)
            },
            doc: None,
            place: Some(crate::nav::Place {
                path: Some(definition.path),
                span: definition.span,
            }),
        })
        .collect()
}
