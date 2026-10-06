//! The casts of an Eloquent model, `$casts` and what `casts()` returns: each key is an attribute,
//! most often a column of the model's table, and each value written as a string is a cast the model
//! knows by name (`datetime`, `decimal:2`) or a class with its arguments after a colon.

use php_index::framework::eloquent;
use php_index::framework::keys::KeyKind;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::keys::KeyString;
use crate::ast::{range_of, text_of};
use crate::context::FileContext;
use crate::infer::Analyzer;

/// Whether an array is the one a `$casts` property holds or a `casts()` method returns.
fn holds_casts(array: &SyntaxNode) -> bool {
    let Some(parent) = array.parent() else {
        return false;
    };
    match parent.kind() {
        PROPERTY_ELEMENT => parent
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .any(|token| token.kind() == VARIABLE && token.text() == "$casts"),
        RETURN_STATEMENT => array
            .ancestors()
            .find(|node| matches!(node.kind(), METHOD_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
            .filter(|node| node.kind() == METHOD_DECLARATION)
            .and_then(|method| method.children().find(|child| child.kind() == NAME))
            .is_some_and(|name| text_of(&name) == "casts"),
        _ => false,
    }
}

/// The table of the model whose casts an array is, when it is that array.
fn model_table(analyzer: &Analyzer<'_>, array: &SyntaxNode) -> Option<String> {
    if !holds_casts(array) {
        return None;
    }
    let class = analyzer.class.as_ref().filter(|class| !class.anonymous)?;
    eloquent::table(analyzer.index, &class.name)
}

/// The item a literal is the key or the value of, with whether it is the key.
fn item_of(literal: &SyntaxNode) -> Option<(SyntaxNode, bool)> {
    let item = literal.parent().filter(|node| node.kind() == ARRAY_ITEM)?;
    let parts: Vec<SyntaxNode> = item.children().collect();
    let [key, value] = parts.as_slice() else {
        return None;
    };
    let is_key = key == literal;
    (is_key || value == literal).then_some((item, is_key))
}

/// A key of a model's casts, which names a column of its table.
pub fn cast_key(
    analyzer: &Analyzer<'_>,
    literal: &SyntaxNode,
    value: &str,
    span: php_index::Span,
) -> Option<KeyString> {
    if !analyzer.index.frameworks().eloquent {
        return None;
    }
    let (item, is_key) = item_of(literal)?;
    if !is_key {
        return None;
    }
    let table = model_table(analyzer, &item.parent()?)?;
    Some(KeyString {
        kind: KeyKind::Column,
        value: value.to_string(),
        range: range_of(span.start, span.end),
        scope: Some(table),
        // A cast may as well name an attribute an accessor makes up.
        guarded: true,
    })
}

/// Every cast of the file written as a string, with the range of its text.
pub fn cast_values(ctx: &FileContext<'_>) -> Vec<(String, TextRange)> {
    if !ctx.index.frameworks().eloquent {
        return Vec::new();
    }
    ctx.root
        .descendants()
        .filter(|node| node.kind() == LITERAL)
        .filter_map(|literal| cast_value(ctx, &literal))
        .collect()
}

fn cast_value(ctx: &FileContext<'_>, literal: &SyntaxNode) -> Option<(String, TextRange)> {
    let (value, span) = php_index::test_facts::string_value(literal)?;
    let (item, is_key) = item_of(literal)?;
    let array = item.parent()?;
    if is_key || !holds_casts(&array) {
        return None;
    }
    model_table(&ctx.analyzer(literal), &array)?;
    Some((value, range_of(span.start, span.end)))
}

/// The cast under an offset, when it is written as a string.
pub fn cast_value_at(ctx: &FileContext<'_>, offset: u32) -> Option<(String, TextRange)> {
    let literal = crate::ast::node_at(&ctx.root, offset)
        .ancestors()
        .find(|node| node.kind() == LITERAL)?;
    let (value, range) = cast_value(ctx, &literal)?;
    (u32::from(range.start()) <= offset && offset <= u32::from(range.end())).then_some((value, range))
}
