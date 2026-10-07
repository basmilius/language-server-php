//! DQL, Doctrine's query language, in the strings that hold it. A whole statement, the argument of
//! `createQuery()`, names its entities and gives them aliases itself; a part of one, an argument of a
//! query builder (`->andWhere('u.email = :email')`), uses the aliases the builder's `from()`, its
//! joins and the repository's `createQueryBuilder('u')` give in the same function. `u.email` names a
//! field of the alias's entity. A statement may be cut in pieces joined with `.`, which are read as
//! one, with `User::class` standing for the name it gives.

use std::collections::HashMap;

use php_index::framework::overlay::{Marker, is_marked, markers_for};
use php_index::framework::symfony::doctrine::{Entities, FieldKind, entity_of_repository};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::keys::callees_of;
use crate::ast::range_of;
use crate::context::FileContext;
use crate::infer::Analyzer;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// The field after `alias.`, of the alias's entity; empty right after the dot.
    Field {
        entity: String,
        name: String,
        range: TextRange,
    },
    /// A class a statement names: an entity after `FROM`, `JOIN`, `UPDATE` or `DELETE`, or the class
    /// of `NEW` and `INSTANCE OF`.
    Class { name: String, range: TextRange },
}

impl Part {
    pub fn range(&self) -> TextRange {
        match self {
            Part::Field { range, .. } | Part::Class { range, .. } => *range,
        }
    }
}

/// A word of DQL: a name, a path like `u.email` or `App\Entity\User`, or what stands between them.
#[derive(Clone, Debug)]
enum Token {
    Word {
        text: String,
        start: Option<u32>,
    },
    /// A class the code names with `::class` between two pieces.
    Class(String),
    Punct,
    /// Code whose value is not known, between two pieces.
    Hole,
}

/// The pieces of the expression an argument is, in order, as tokens.
fn tokens_of(analyzer: &Analyzer<'_>, expression: &SyntaxNode, out: &mut Vec<Token>) {
    match expression.kind() {
        LITERAL => match php_index::test_facts::string_value(expression) {
            Some((text, span)) if !text.contains('$') => lex(&text, span.start, out),
            _ => out.push(Token::Hole),
        },
        BINARY_EXPR if is_concat(expression) => {
            for child in expression.children() {
                tokens_of(analyzer, &child, out);
            }
        }
        PAREN_EXPR => {
            for child in expression.children() {
                tokens_of(analyzer, &child, out);
            }
        }
        SCOPED_ACCESS_EXPR => match php_index::test_facts::class_constant(expression, &analyzer.resolver) {
            Some(class) => out.push(Token::Class(class)),
            None => out.push(Token::Hole),
        },
        _ => out.push(Token::Hole),
    }
}

fn is_concat(node: &SyntaxNode) -> bool {
    node.children_with_tokens()
        .filter_map(|element| element.into_token())
        .any(|token| token.kind() == DOT)
}

fn lex(text: &str, base: u32, out: &mut Vec<Token>) {
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte.is_ascii_whitespace() {
            at += 1;
        } else if byte == b'\'' || byte == b'"' {
            // A string of the query, which names nothing.
            at += 1;
            while at < bytes.len() && bytes[at] != byte {
                at += 1;
            }
            at += 1;
            out.push(Token::Hole);
        } else if byte == b':' || byte == b'?' {
            at += 1;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_') {
                at += 1;
            }
            out.push(Token::Hole);
        } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'\\' {
            let start = at;
            while at < bytes.len() && (bytes[at].is_ascii_alphanumeric() || matches!(bytes[at], b'_' | b'\\' | b'.')) {
                at += 1;
            }
            out.push(Token::Word {
                text: text[start..at].to_string(),
                start: Some(base + start as u32),
            });
        } else {
            out.push(Token::Punct);
            at += 1;
        }
    }
}

/// What the aliases of a query stand for: an entity, or `None` when the code gives an alias to more
/// than one or to something the index does not know.
type Aliases = HashMap<String, Option<String>>;

fn give(aliases: &mut Aliases, alias: &str, entity: Option<String>) {
    match aliases.get(alias) {
        Some(known)
            if known.as_ref().map(|name| name.to_ascii_lowercase())
                != entity.as_ref().map(|name| name.to_ascii_lowercase()) =>
        {
            aliases.insert(alias.to_string(), None);
        }
        Some(_) => {}
        None => {
            aliases.insert(alias.to_string(), entity);
        }
    }
}

/// The class a name of DQL is, as the index knows it.
fn class_named(analyzer: &Analyzer<'_>, name: &str) -> Option<String> {
    let name = name.replace("\\\\", "\\");
    let class = analyzer.index.class(name.trim_start_matches('\\'))?;
    Some(class.decl.name.clone())
}

/// The entity a relation of an entity leads to.
fn relation_target(analyzer: &Analyzer<'_>, entity: &str, field: &str) -> Option<String> {
    let entities = analyzer.index.section::<Entities>();
    let found = entities
        .find(entity)?
        .fields
        .iter()
        .find(|candidate| candidate.name == field)?;
    match &found.kind {
        FieldKind::Relation { target, .. } => target.clone(),
        _ => None,
    }
}

/// The entity a join names: `u.posts` leads where the relation does, else it is a class.
fn joined(analyzer: &Analyzer<'_>, aliases: &Aliases, join: &str) -> Option<Option<String>> {
    match join.split_once('.') {
        Some((alias, field)) => {
            let entity = aliases.get(alias)?.as_ref()?;
            Some(relation_target(analyzer, entity, field))
        }
        None => Some(class_named(analyzer, join)),
    }
}

const NOT_ALIASES: &[&str] = &[
    "where", "join", "left", "inner", "with", "on", "order", "group", "having", "index", "set", "and", "or",
];

fn is_keyword(token: &Token, keyword: &str) -> bool {
    matches!(token, Token::Word { text, .. } if text.eq_ignore_ascii_case(keyword))
}

/// The alias after the name at `at`, skipping `AS`.
fn alias_after(tokens: &[Token], at: usize) -> Option<String> {
    let mut next = at + 1;
    if tokens.get(next).is_some_and(|token| is_keyword(token, "as")) {
        next += 1;
    }
    match tokens.get(next)? {
        Token::Word { text, .. }
            if !text.contains(['.', '\\']) && !NOT_ALIASES.contains(&text.to_ascii_lowercase().as_str()) =>
        {
            Some(text.clone())
        }
        _ => None,
    }
}

/// The aliases a statement gives in its `FROM`, `JOIN`, `UPDATE` and `DELETE`, and the classes it names.
fn read_statement(analyzer: &Analyzer<'_>, tokens: &[Token], aliases: &mut Aliases, parts: &mut Vec<Part>) {
    let mut joins: Vec<(String, String)> = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        let names_class = ["from", "join", "update", "delete", "new"]
            .iter()
            .any(|keyword| is_keyword(token, keyword))
            || is_keyword(token, "of") && at > 0 && is_keyword(&tokens[at - 1], "instance");
        if !names_class {
            continue;
        }
        let gives_alias = !is_keyword(token, "new") && !is_keyword(token, "of");
        match tokens.get(at + 1) {
            Some(Token::Class(name)) => {
                if gives_alias {
                    if let Some(alias) = alias_after(tokens, at + 1) {
                        give(aliases, &alias, class_named(analyzer, name));
                    }
                }
            }
            Some(Token::Word { text, .. }) if text.contains('.') && !text.contains('\\') => {
                if let Some(alias) = alias_after(tokens, at + 1).filter(|_| gives_alias) {
                    joins.push((text.clone(), alias));
                }
            }
            Some(Token::Word { text, start }) => {
                if is_keyword(&tokens[at + 1], "from") {
                    continue;
                }
                let class = class_named(analyzer, text);
                if let (Some(class), Some(start)) = (&class, start) {
                    parts.push(Part::Class {
                        name: class.clone(),
                        range: range_of(*start, start + text.len() as u32),
                    });
                }
                if gives_alias {
                    if let Some(alias) = alias_after(tokens, at + 1) {
                        give(aliases, &alias, class);
                    }
                }
            }
            _ => {}
        }
    }
    settle_joins(analyzer, aliases, joins);
}

/// Gives the aliases of joins whose path starts at an alias that may itself come from a join.
fn settle_joins(analyzer: &Analyzer<'_>, aliases: &mut Aliases, mut joins: Vec<(String, String)>) {
    loop {
        let before = joins.len();
        joins.retain(|(join, alias)| match joined(analyzer, aliases, join) {
            Some(entity) => {
                give(aliases, alias, entity);
                false
            }
            None => true,
        });
        if joins.is_empty() || joins.len() == before {
            break;
        }
    }
    for (_, alias) in joins {
        give(aliases, &alias, None);
    }
}

/// The fields the paths of a query name, `u.email` and `u.` with nothing after it yet.
fn read_paths(tokens: &[Token], aliases: &Aliases, parts: &mut Vec<Part>) {
    for token in tokens {
        let Token::Word {
            text,
            start: Some(start),
        } = token
        else {
            continue;
        };
        let Some((alias, rest)) = text.split_once('.') else {
            continue;
        };
        let Some(Some(entity)) = aliases.get(alias) else {
            continue;
        };
        let name = rest.split('.').next().unwrap_or_default();
        let from = start + alias.len() as u32 + 1;
        parts.push(Part::Field {
            entity: entity.clone(),
            name: name.to_string(),
            range: range_of(from, from + name.len() as u32),
        });
    }
}

/// How a call reads one of its arguments: as a whole statement or as a part of one.
fn role_of(analyzer: &Analyzer<'_>, argument: &SyntaxNode) -> Option<bool> {
    let list = argument.parent().filter(|list| list.kind() == ARGUMENT_LIST)?;
    let call = list.parent().filter(|call| call.kind() == CALL_EXPR)?;
    let position = list
        .children()
        .filter(|child| child.kind() == ARGUMENT)
        .position(|child| &child == argument)?;
    callees_of(analyzer, &call).into_iter().find_map(|callee| {
        markers_for(
            analyzer.index,
            callee.declaring.as_deref(),
            callee.receiver.as_deref(),
            &callee.method,
        )
        .into_iter()
        .find_map(|marker| match marker {
            Marker::Dql {
                position: wanted,
                whole,
            } if wanted.is_none_or(|wanted| wanted == position) => Some(whole),
            _ => None,
        })
    })
}

/// Whether a call reads an argument as DQL, which reads like SQL and is none.
pub(crate) fn is_dql_argument(analyzer: &Analyzer<'_>, argument: &SyntaxNode) -> bool {
    analyzer.index.frameworks().any() && role_of(analyzer, argument).is_some()
}

/// The argument whose expression a literal is a piece of.
fn argument_around(literal: &SyntaxNode) -> Option<SyntaxNode> {
    let mut node = literal.clone();
    loop {
        let parent = node.parent()?;
        match parent.kind() {
            ARGUMENT => return Some(parent),
            BINARY_EXPR if is_concat(&parent) => node = parent,
            PAREN_EXPR => node = parent,
            _ => return None,
        }
    }
}

/// The aliases the query builders of a function give: `from()`, the joins, and the repository's
/// `createQueryBuilder()`.
fn builder_aliases(analyzer: &Analyzer<'_>, scope: &SyntaxNode) -> Aliases {
    let mut aliases = Aliases::new();
    let mut joins: Vec<(String, String)> = Vec::new();
    for call in scope.descendants().filter(|node| node.kind() == CALL_EXPR) {
        let Some(name) = call
            .children()
            .next()
            .and_then(|callee| callee.descendants().filter(|node| node.kind() == NAME).last())
        else {
            continue;
        };
        if !is_marked(analyzer.index, &name.text().to_string()) {
            continue;
        }
        let arguments: Vec<SyntaxNode> = call
            .children()
            .find(|child| child.kind() == ARGUMENT_LIST)
            .map(|list| list.children().filter(|child| child.kind() == ARGUMENT).collect())
            .unwrap_or_default();
        let value = |position: usize| arguments.get(position).and_then(|argument| argument.children().last());
        let text = |position: usize| {
            value(position)
                .and_then(|node| php_index::test_facts::string_value(&node))
                .map(|(text, _)| text)
        };
        let class = |position: usize| {
            let node = value(position)?;
            match php_index::test_facts::class_constant(&node, &analyzer.resolver) {
                Some(class) => Some(class_named(analyzer, &class)),
                None => php_index::test_facts::string_value(&node).map(|(text, _)| class_named(analyzer, &text)),
            }
        };
        for callee in callees_of(analyzer, &call) {
            let markers = markers_for(
                analyzer.index,
                callee.declaring.as_deref(),
                callee.receiver.as_deref(),
                &callee.method,
            );
            for marker in markers {
                match marker {
                    Marker::DqlFrom { entity, alias } => {
                        if let (Some(entity), Some(alias)) = (class(entity), text(alias)) {
                            give(&mut aliases, &alias, entity);
                        }
                    }
                    Marker::DqlJoin { join, alias } => {
                        let Some(alias) = text(alias) else {
                            continue;
                        };
                        match (text(join), class(join)) {
                            (Some(path), _) if path.contains('.') && !path.contains('\\') => joins.push((path, alias)),
                            (_, Some(entity)) => give(&mut aliases, &alias, entity),
                            _ => give(&mut aliases, &alias, None),
                        }
                    }
                    Marker::DqlAlias { alias } => {
                        let Some(alias) = text(alias) else {
                            continue;
                        };
                        let entity = callee
                            .receiver_type
                            .as_ref()
                            .and_then(|receiver| entity_of_repository(analyzer.index, receiver));
                        give(&mut aliases, &alias, entity);
                    }
                    _ => {}
                }
            }
        }
    }
    settle_joins(analyzer, &mut aliases, joins);
    aliases
}

/// The function-like node around a node, or the file.
fn scope_of(node: &SyntaxNode) -> SyntaxNode {
    node.ancestors()
        .find(|ancestor| matches!(ancestor.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION | SOURCE_FILE))
        .unwrap_or_else(|| node.clone())
}

/// The parts of the DQL an argument holds, when a call reads it as DQL.
fn parts_of_argument(
    analyzer: &Analyzer<'_>,
    argument: &SyntaxNode,
    builders: &mut HashMap<TextRange, Aliases>,
) -> Option<Vec<Part>> {
    let whole = role_of(analyzer, argument)?;
    let expression = argument.children().last()?;
    let mut tokens = Vec::new();
    tokens_of(analyzer, &expression, &mut tokens);
    let mut aliases = if whole {
        Aliases::new()
    } else {
        let scope = scope_of(argument);
        builders
            .entry(scope.text_range())
            .or_insert_with(|| builder_aliases(analyzer, &scope))
            .clone()
    };
    let mut parts = Vec::new();
    // A part may hold a subquery with its own `FROM`.
    read_statement(analyzer, &tokens, &mut aliases, &mut parts);
    read_paths(&tokens, &aliases, &mut parts);
    Some(parts)
}

/// The part of DQL under an offset.
pub fn part_at(ctx: &FileContext<'_>, offset: u32) -> Option<Part> {
    if !ctx.index.frameworks().symfony {
        return None;
    }
    let literal = crate::ast::node_at(&ctx.root, offset)
        .ancestors()
        .find(|node| node.kind() == LITERAL)?;
    let argument = argument_around(&literal)?;
    let analyzer = ctx.analyzer(&literal);
    parts_of_argument(&analyzer, &argument, &mut HashMap::new())?
        .into_iter()
        .find(|part| {
            let range = part.range();
            u32::from(range.start()) <= offset && offset <= u32::from(range.end())
        })
}

/// Every part of DQL in a file.
pub fn parts_in(ctx: &FileContext<'_>) -> Vec<Part> {
    if !ctx.index.frameworks().symfony {
        return Vec::new();
    }
    let mut builders = HashMap::new();
    let mut out = Vec::new();
    for argument in ctx.root.descendants().filter(|node| node.kind() == ARGUMENT) {
        let holds_string = argument
            .children()
            .last()
            .is_some_and(|expression| matches!(expression.kind(), LITERAL | BINARY_EXPR | PAREN_EXPR));
        if !holds_string || !argument.descendants().any(|node| node.kind() == LITERAL) {
            continue;
        }
        let analyzer = ctx.analyzer(&argument);
        if let Some(parts) = parts_of_argument(&analyzer, &argument, &mut builders) {
            out.extend(parts);
        }
    }
    out
}

/// The parts of DQL in the literals of a file that hold a word, for a search.
pub fn parts_with(ctx: &FileContext<'_>, word: &str) -> Vec<Part> {
    if !ctx.index.frameworks().symfony {
        return Vec::new();
    }
    let mut builders = HashMap::new();
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for literal in ctx.root.descendants().filter(|node| node.kind() == LITERAL) {
        if php_index::test_facts::string_value(&literal).is_none_or(|(text, _)| !text.contains(word)) {
            continue;
        }
        let Some(argument) = argument_around(&literal) else {
            continue;
        };
        if seen.contains(&argument) {
            continue;
        }
        seen.push(argument.clone());
        let analyzer = ctx.analyzer(&literal);
        if let Some(parts) = parts_of_argument(&analyzer, &argument, &mut builders) {
            out.extend(parts);
        }
    }
    out
}
