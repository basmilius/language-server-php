//! The tables a part of a query sees: the ones its query builder names in the chain of calls it
//! is in (`from()`, `table()`, the joins), the table of the model the query is of, and those of the
//! query around a closure it is passed to (`->join('t', fn ($q) => $q->on(...))`).

use php_index::Type;
use php_index::framework::raxos::orm::{Models, model_of as raxos_model};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;
use sql_embed::ScopeTable;

use super::Finder;
use crate::ast::text_of;

/// How many closures out a part looks for the tables of its query.
const MOST_NESTING: u32 = 4;

/// The table of a model of the project: what Raxos' `#[Table]` says, on the class or a parent, or
/// the table of an Eloquent model.
pub(crate) fn model_table(index: &php_index::Index, class: &str) -> Option<String> {
    if index.frameworks().raxos {
        let models = index.section::<Models>();
        let mut current = Some(class.to_string());
        let mut steps = 0;
        while let Some(name) = current {
            if let Some(table) = models.get(&name).and_then(|info| info.table.clone()) {
                return Some(table);
            }
            steps += 1;
            if steps > 8 {
                break;
            }
            current = index.class(&name).and_then(|found| {
                found
                    .decl
                    .extends
                    .first()
                    .and_then(|ty| ty.class_names().first().map(|name| name.to_string()))
            });
        }
    }
    if index.frameworks().eloquent {
        return php_index::framework::eloquent::table(index, class);
    }
    None
}

/// The table of the model a query type is of: `QueryInterface<User>`, `Builder<User>`.
fn table_of_type(finder: &Finder, node: &SyntaxNode, ty: &Type) -> Option<String> {
    let index = finder.ctx.index;
    if index.frameworks().raxos {
        if let Some(model) = raxos_model(index, ty) {
            return model_table(index, &model);
        }
    }
    if index.frameworks().eloquent {
        let analyzer = finder.ctx.analyzer(node);
        if let Some(model) = crate::frameworks::relations::model_of(&analyzer, ty) {
            return php_index::framework::eloquent::table(index, &model);
        }
    }
    None
}

fn unquote(name: &str) -> &str {
    name.trim_matches(['`', '"', '[', ']'])
}

fn is_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_alphanumeric() || matches!(character, '_' | '$'))
}

/// A table as a builder takes it: `users`, `app.users`, `users u`, `users as u`.
pub(crate) fn table_named(text: &str) -> Option<ScopeTable> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let (name, alias) = match words.as_slice() {
        [name] => (*name, None),
        [name, alias] => (*name, Some(*alias)),
        [name, keyword, alias] if keyword.eq_ignore_ascii_case("as") => (*name, Some(*alias)),
        _ => return None,
    };
    let (schema, name) = match name.split_once('.') {
        Some((schema, name)) => (Some(unquote(schema)), unquote(name)),
        None => (None, unquote(name)),
    };
    if !is_name(name) || schema.is_some_and(|schema| !is_name(schema)) {
        return None;
    }
    let mut table = ScopeTable::new(name);
    if let Some(schema) = schema {
        table = table.with_schema(schema);
    }
    if let Some(alias) = alias.map(unquote).filter(|alias| is_name(alias)) {
        table = table.with_alias(alias);
    }
    Some(table)
}

/// The value of a table argument: a string, or what a model's `table()` gives.
fn table_argument(finder: &Finder, expression: &SyntaxNode) -> Option<ScopeTable> {
    if let Some(text) = crate::infer::literal_string(expression) {
        return table_named(&text);
    }
    if expression.kind() == CALL_EXPR {
        let callee = expression.children().next()?;
        if callee.kind() != SCOPED_ACCESS_EXPR {
            return None;
        }
        let mut parts = callee.children().filter(|child| child.kind() == NAME);
        let (qualifier, method) = (parts.next()?, parts.next()?);
        if !text_of(&method).eq_ignore_ascii_case("table") {
            return None;
        }
        let analyzer = finder.ctx.analyzer(expression);
        let class = analyzer.resolver.resolve_class(&text_of(&qualifier));
        return model_table(finder.ctx.index, &class).map(ScopeTable::new);
    }
    None
}

/// The arguments of a call by their place: positional ones in order, named ones by name.
pub(crate) fn arguments(call: &SyntaxNode) -> Vec<(Option<String>, SyntaxNode)> {
    let Some(list) = call.children().find(|child| child.kind() == ARGUMENT_LIST) else {
        return Vec::new();
    };
    crate::infer::arguments(call)
        .into_iter()
        .zip(list.children().filter(|child| child.kind() == ARGUMENT))
        .map(|(argument, node)| (argument.name, node))
        .collect()
}

/// The table and alias a call of a builder adds to its query.
fn table_of_call(finder: &Finder, call: &SyntaxNode) -> Option<ScopeTable> {
    let sinks = finder.matches(call);
    let sink = sinks.iter().find(|sink| sink.table.is_some())?;
    let arguments = arguments(call);
    let count = arguments.len();
    let mut table = None;
    let mut alias = None;
    for (position, (name, argument)) in arguments.iter().enumerate() {
        let Some(place) = sink.place_of(position, name.as_deref(), count) else {
            continue;
        };
        let Some(expression) = argument.children().last() else {
            continue;
        };
        if sink.table_at(place) && table.is_none() {
            table = table_argument(finder, &expression);
        } else if sink.alias_at(place) {
            alias = crate::infer::literal_string(&expression).filter(|alias| is_name(alias));
        }
    }
    let table = table?;
    Some(match alias {
        Some(alias) if table.alias.is_none() => table.with_alias(alias),
        _ => table,
    })
}

/// The calls of the chain a call is in, outermost first, and what the chain starts from.
fn chain(call: &SyntaxNode) -> (Vec<SyntaxNode>, Option<SyntaxNode>) {
    let mut top = call.clone();
    while let Some(fetch) = top.parent().filter(|parent| parent.kind() == PROPERTY_FETCH_EXPR) {
        let outer = fetch
            .parent()
            .filter(|outer| outer.kind() == CALL_EXPR && outer.children().next().as_ref() == Some(&fetch));
        match outer {
            Some(outer) if fetch.children().next().as_ref() == Some(&top) => top = outer,
            _ => break,
        }
    }
    let mut calls = Vec::new();
    let mut node = top;
    loop {
        if node.kind() != CALL_EXPR {
            return (calls, Some(node));
        }
        calls.push(node.clone());
        let Some(callee) = node.children().next() else {
            return (calls, None);
        };
        match callee.kind() {
            PROPERTY_FETCH_EXPR => match callee.children().next() {
                Some(object) => node = object,
                None => return (calls, None),
            },
            SCOPED_ACCESS_EXPR => return (calls, callee.children().next()),
            _ => return (calls, None),
        }
    }
}

fn push(tables: &mut Vec<ScopeTable>, table: ScopeTable) {
    if !tables.contains(&table) {
        tables.push(table);
    }
}

/// The tables of the query a call that takes part of it belongs to.
pub(crate) fn tables(finder: &Finder, call: &SyntaxNode) -> Vec<ScopeTable> {
    let mut out = Vec::new();
    collect(finder, call, &mut out, 0);
    out
}

fn collect(finder: &Finder, call: &SyntaxNode, out: &mut Vec<ScopeTable>, depth: u32) {
    let (calls, base) = chain(call);
    for link in calls.iter().rev() {
        if let Some(table) = table_of_call(finder, link) {
            push(out, table);
        }
    }
    let analyzer = finder.ctx.analyzer(call);
    let env = analyzer.env_around(call);
    let mut model_table = None;
    match &base {
        Some(base) if base.kind() == NAME => {
            let class = analyzer.resolver.resolve_class(&text_of(base));
            model_table = self::model_table(finder.ctx.index, &class);
        }
        Some(base) => {
            let ty = analyzer.type_of(base, &env);
            model_table = table_of_type(finder, base, &ty);
            if base.kind() == VARIABLE_EXPR {
                for table in variable_tables(finder, base) {
                    push(out, table);
                }
            }
        }
        None => {}
    }
    if model_table.is_none() {
        if let Some(object) = call
            .children()
            .next()
            .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
            .and_then(|callee| callee.children().next())
        {
            let ty = analyzer.type_of(&object, &env);
            model_table = table_of_type(finder, &object, &ty);
        }
    }
    if let Some(table) = model_table {
        if !out.iter().any(|known| known.name.eq_ignore_ascii_case(&table)) {
            out.insert(0, ScopeTable::new(table));
        }
    }
    if depth >= MOST_NESTING {
        return;
    }
    let top = calls.first().cloned().unwrap_or_else(|| call.clone());
    let outer = top
        .ancestors()
        .find(|ancestor| matches!(ancestor.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
        .and_then(|closure| closure.parent().filter(|parent| parent.kind() == ARGUMENT))
        .and_then(|argument| argument.parent())
        .and_then(|list| list.parent())
        .filter(|call| call.kind() == CALL_EXPR);
    if let Some(outer) = outer {
        collect(finder, &outer, out, depth + 1);
    }
}

/// The tables the other chains on the same variable add in its function: a builder set up over
/// several statements.
fn variable_tables(finder: &Finder, variable: &SyntaxNode) -> Vec<ScopeTable> {
    let name = text_of(variable);
    let scope = variable
        .ancestors()
        .find(|ancestor| crate::ast::is_function_like(ancestor.kind()))
        .unwrap_or_else(|| finder.ctx.root.clone());
    let mut found: Vec<(u32, ScopeTable)> = Vec::new();
    for call in scope.descendants().filter(|node| node.kind() == CALL_EXPR) {
        let Some(callee) = call
            .children()
            .next()
            .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
        else {
            continue;
        };
        let names_table = callee
            .children()
            .filter(|child| child.kind() == NAME)
            .last()
            .is_some_and(|method| {
                super::sinks::named(&text_of(&method))
                    .iter()
                    .any(|sink| sink.table.is_some())
            });
        if !names_table {
            continue;
        }
        let (_, base) = chain(&call);
        if base.is_some_and(|base| base.kind() == VARIABLE_EXPR && text_of(&base) == name) {
            if let Some(table) = table_of_call(finder, &call) {
                found.push((u32::from(callee.text_range().end()), table));
            }
        }
    }
    // In the order the calls are made, which preorder is not for a chain.
    found.sort_by_key(|(at, _)| *at);
    let mut out = Vec::new();
    for (_, table) in found {
        push(&mut out, table);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_table_as_a_builder_writes_it() {
        assert_eq!(table_named("users"), Some(ScopeTable::new("users")));
        assert_eq!(table_named("users u"), Some(ScopeTable::new("users").with_alias("u")));
        assert_eq!(
            table_named("app.users AS u"),
            Some(ScopeTable::new("users").with_schema("app").with_alias("u"))
        );
        assert_eq!(table_named("`users`"), Some(ScopeTable::new("users")));
        assert_eq!(table_named("select * from users"), None);
        assert_eq!(table_named(""), None);
    }
}
