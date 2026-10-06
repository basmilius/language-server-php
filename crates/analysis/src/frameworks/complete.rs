//! Completion inside the strings that name a config key, a route, a view and the like.

use php_index::Index;
use php_index::framework::keys::{KeyKind, candidates};
use php_syntax::SyntaxNode;

use super::keys::key_at;
use crate::completion::{CompletionItem, CompletionList, CompletionOptions, ItemKind, TextEdit, match_score};
use crate::infer::Analyzer;

fn item_kind(kind: KeyKind) -> ItemKind {
    match kind {
        KeyKind::Config => ItemKind::Property,
        KeyKind::Route => ItemKind::Method,
        KeyKind::View => ItemKind::Module,
        KeyKind::Translation => ItemKind::Constant,
        KeyKind::Env => ItemKind::Variable,
        KeyKind::Ability => ItemKind::Keyword,
        KeyKind::Field => ItemKind::Property,
        KeyKind::Component => ItemKind::Class,
        KeyKind::Service => ItemKind::Class,
        KeyKind::Parameter => ItemKind::Constant,
        KeyKind::Template => ItemKind::Module,
        KeyKind::Event => ItemKind::Constant,
        KeyKind::EntityField => ItemKind::Property,
        KeyKind::Section | KeyKind::Stack | KeyKind::Block => ItemKind::Module,
        KeyKind::Slot | KeyKind::Attribute => ItemKind::Property,
        KeyKind::Relation => ItemKind::Method,
        KeyKind::Table | KeyKind::Column => ItemKind::Property,
    }
}

/// The completions for the string around an offset, `None` when it is not one that names something.
pub fn complete_key(
    index: &Index,
    root: &SyntaxNode,
    text: &str,
    offset: u32,
    options: CompletionOptions,
) -> Option<CompletionList> {
    if !index.frameworks().any() {
        return None;
    }
    let analyzer = Analyzer::new(index, root, offset);
    let Some(found) = key_at(&analyzer, offset) else {
        return rule_items(&analyzer, text, offset, options)
            .or_else(|| cast_items(&analyzer, text, offset, options))
            .or_else(|| dql_items(&analyzer, text, offset, options));
    };
    if found.kind == KeyKind::Relation {
        // Each segment of `posts.comments` completes from the model the one before leads to.
        let segment = super::relations::segment_at(index, &found, offset)?;
        let start = u32::from(segment.range.start());
        let typed = text.get(start as usize..offset as usize)?;
        return Some(key_items(
            index,
            KeyKind::Relation,
            Some(&segment.model),
            typed,
            (start, u32::from(segment.range.end())),
            options,
        ));
    }
    let start = u32::from(found.range.start());
    let typed = text.get(start as usize..offset as usize)?;
    Some(key_items(
        index,
        found.kind,
        found.scope.as_deref(),
        typed,
        (start, u32::from(found.range.end())),
        options,
    ))
}

/// The rules, tables, columns or fields a validation rule string can hold where it is typed.
fn rule_items(analyzer: &Analyzer<'_>, text: &str, offset: u32, options: CompletionOptions) -> Option<CompletionList> {
    use super::rules::{Part, part_at, rule_names};
    let ctx = crate::context::FileContext::new(analyzer.index, &analyzer.root);
    let (part, set) = part_at(&ctx, offset)?;
    let range = part.range();
    let (start, end) = (u32::from(range.start()), u32::from(range.end()));
    let typed = text.get(start as usize..offset as usize)?;
    let names: Vec<(String, ItemKind)> = match &part {
        Part::Rule { .. } => rule_names(analyzer)
            .into_iter()
            .map(|name| (name, ItemKind::Keyword))
            .collect(),
        Part::Table { .. } => {
            return Some(key_items(
                analyzer.index,
                KeyKind::Table,
                None,
                typed,
                (start, end),
                options,
            ));
        }
        Part::Column { table, .. } => {
            return Some(key_items(
                analyzer.index,
                KeyKind::Column,
                Some(table),
                typed,
                (start, end),
                options,
            ));
        }
        Part::Field { .. } => set
            .fields
            .iter()
            .map(|(name, _)| (name.clone(), ItemKind::Property))
            .collect(),
    };
    Some(name_items(names, typed, (start, end), "validation rule", options))
}

/// The casts a model takes by name, in the string of a cast.
fn cast_items(analyzer: &Analyzer<'_>, text: &str, offset: u32, options: CompletionOptions) -> Option<CompletionList> {
    let ctx = crate::context::FileContext::new(analyzer.index, &analyzer.root);
    let (_, range) = super::casts::cast_value_at(&ctx, offset)?;
    let (start, end) = (u32::from(range.start()), u32::from(range.end()));
    let typed = text.get(start as usize..offset as usize)?;
    let names = php_index::framework::eloquent::primitive_casts(analyzer.index)?
        .into_iter()
        // The model turns `date:Y-m-d` into these itself; nobody writes them.
        .filter(|name| !name.contains("custom_"))
        .map(|name| (name, ItemKind::Keyword))
        .collect();
    Some(name_items(names, typed, (start, end), "cast", options))
}

/// The fields of the entity an alias of DQL stands for, after its dot.
fn dql_items(analyzer: &Analyzer<'_>, text: &str, offset: u32, options: CompletionOptions) -> Option<CompletionList> {
    let ctx = crate::context::FileContext::new(analyzer.index, &analyzer.root);
    let super::dql::Part::Field { entity, range, .. } = super::dql::part_at(&ctx, offset)? else {
        return None;
    };
    let (start, end) = (u32::from(range.start()), u32::from(range.end()));
    let typed = text.get(start as usize..offset as usize)?;
    Some(key_items(
        analyzer.index,
        KeyKind::EntityField,
        Some(&entity),
        typed,
        (start, end),
        options,
    ))
}

/// Plain names that fit what was typed, each replacing the text from `start` to `end`.
fn name_items(
    names: Vec<(String, ItemKind)>,
    typed: &str,
    (start, end): (u32, u32),
    description: &str,
    options: CompletionOptions,
) -> CompletionList {
    let mut items: Vec<(u8, CompletionItem)> = names
        .into_iter()
        .filter_map(|(name, kind)| {
            let score = match_score(&name, typed)?;
            Some((
                score,
                CompletionItem {
                    label: name.clone(),
                    kind,
                    detail: None,
                    description: Some(description.to_string()),
                    edit: TextEdit {
                        start,
                        end,
                        new_text: name.clone(),
                    },
                    additional_edits: Vec::new(),
                    sort_text: name.clone(),
                    filter_text: Some(name),
                    deprecated: false,
                    data: None,
                },
            ))
        })
        .collect();
    items.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.label.cmp(&right.1.label)));
    let incomplete = items.len() > options.limit;
    CompletionList {
        items: items.into_iter().take(options.limit).map(|(_, item)| item).collect(),
        incomplete,
    }
}

/// The names of a kind that fit what was typed, each replacing the text from `start` to `end`.
pub(crate) fn key_items(
    index: &Index,
    kind: KeyKind,
    scope: Option<&str>,
    typed: &str,
    (start, end): (u32, u32),
    options: CompletionOptions,
) -> CompletionList {
    let mut items: Vec<(u8, CompletionItem)> = candidates(index, kind, scope)
        .into_iter()
        .filter_map(|candidate| {
            let score = match_score(&candidate.key, typed)?;
            Some((
                score,
                CompletionItem {
                    label: candidate.key.clone(),
                    kind: item_kind(kind),
                    detail: candidate.detail,
                    description: Some(kind.label().to_string()),
                    edit: TextEdit {
                        start,
                        end,
                        new_text: candidate.key.clone(),
                    },
                    additional_edits: Vec::new(),
                    sort_text: candidate.key.to_ascii_lowercase(),
                    filter_text: Some(candidate.key),
                    deprecated: false,
                    data: None,
                },
            ))
        })
        .collect();
    items.sort_by(|left, right| (left.0, &left.1.sort_text).cmp(&(right.0, &right.1.sort_text)));
    let incomplete = items.len() > options.limit;
    items.truncate(options.limit);
    CompletionList {
        items: items.into_iter().map(|(_, item)| item).collect(),
        incomplete,
    }
}
