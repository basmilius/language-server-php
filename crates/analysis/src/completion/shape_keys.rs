//! The keys of an array shape, `array{date: string, value: float}`, where its value is indexed:
//! between the quotes of `$value['…']` and as a quoted key after a bare `$value[`.

use php_index::types::ShapeField;
use php_index::{Index, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken, TextSize, TokenAtOffset};

use super::{Builder, CompletionItem, CompletionList, CompletionOptions, ItemKind, TextEdit, match_score};
use crate::infer::{Analyzer, Env};

/// The keys for the string around an offset, `None` when the string is not the key of an access to
/// a shape.
pub(super) fn complete_quoted(
    index: &Index,
    root: &SyntaxNode,
    text: &str,
    offset: u32,
    options: CompletionOptions,
) -> Option<CompletionList> {
    let token = match root.token_at_offset(TextSize::from(offset)) {
        TokenAtOffset::Single(token) => token,
        TokenAtOffset::Between(left, right) => {
            [left, right].into_iter().find(|token| token.kind() == STRING_LITERAL)?
        }
        TokenAtOffset::None => return None,
    };
    if token.kind() != STRING_LITERAL {
        return None;
    }
    let literal = token.parent().filter(|node| node.kind() == LITERAL)?;
    let access = literal.parent().filter(|node| node.kind() == INDEX_EXPR)?;
    let base = access.children().next().filter(|base| base != &literal)?;
    let start = u32::from(token.text_range().start()) + 1;
    let end = u32::from(token.text_range().end()).saturating_sub(1);
    if offset < start || offset > end {
        return None;
    }
    let analyzer = Analyzer::new(index, root, offset);
    let env = analyzer.env_at(offset);
    let fields = shape_fields(&analyzer, &env, &base);
    if fields.is_empty() {
        return None;
    }
    let typed = text.get(start as usize..offset as usize)?;
    let mut items: Vec<(u8, CompletionItem)> = fields
        .into_iter()
        .enumerate()
        .filter_map(|(position, (key, field))| {
            let score = match_score(&key, typed)?;
            let item = key_item(
                key.clone(),
                key,
                &field,
                position,
                TextEdit {
                    start,
                    end,
                    new_text: String::new(),
                },
            );
            Some((score, item))
        })
        .collect();
    items.sort_by(|a, b| (a.0, &a.1.sort_text).cmp(&(b.0, &b.1.sort_text)));
    let incomplete = items.len() > options.limit;
    items.truncate(options.limit);
    Some(CompletionList {
        items: items.into_iter().map(|(_, item)| item).collect(),
        incomplete,
    })
}

impl Builder<'_> {
    pub(super) fn shape_keys(&mut self, token: &SyntaxToken) {
        let Some(name) = token.parent().filter(|node| node.kind() == NAME) else {
            return;
        };
        let Some(base) = name
            .parent()
            .filter(|node| node.kind() == INDEX_EXPR)
            .and_then(|access| access.children().next())
            .filter(|base| base != &name)
        else {
            return;
        };
        let typed = self.typed().to_string();
        for (position, (key, field)) in shape_fields(self.analyzer, self.env, &base).into_iter().enumerate() {
            let Some(score) = match_score(&key, &typed) else {
                continue;
            };
            let written = if key.parse::<i64>().is_ok() {
                key.clone()
            } else {
                format!("'{}'", key.replace('\\', "\\\\").replace('\'', "\\'"))
            };
            let item = key_item(key, written, &field, position, self.range_edit(String::new()));
            self.push(score, item);
        }
    }
}

fn key_item(key: String, written: String, field: &ShapeField, position: usize, mut edit: TextEdit) -> CompletionItem {
    edit.new_text = written;
    CompletionItem {
        label: key.clone(),
        kind: ItemKind::Property,
        detail: Some(field.ty.display(true)),
        description: field.optional.then(|| "optional".to_string()),
        edit,
        additional_edits: Vec::new(),
        sort_text: format!("!{position:03}"),
        filter_text: Some(key),
        deprecated: false,
        data: None,
    }
}

/// The named keys of every shape the base can be, each once, in the order they are declared.
fn shape_fields(analyzer: &Analyzer<'_>, env: &Env, base: &SyntaxNode) -> Vec<(String, ShapeField)> {
    let ty = analyzer.type_of(base, env);
    let mut fields: Vec<(String, ShapeField)> = Vec::new();
    for member in ty.members() {
        let Type::Shape(own) = member else {
            continue;
        };
        for field in own {
            if let Some(key) = &field.key {
                if !fields.iter().any(|(seen, _)| seen == key) {
                    fields.push((key.clone(), field.clone()));
                }
            }
        }
    }
    fields
}
