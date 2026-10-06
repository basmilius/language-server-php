//! Strings that name Eloquent relations: `with('posts.comments')`, `whereHas('author')`,
//! `withCount(['posts as published' => ...])`. Each segment of a dotted name is a relation of the
//! model the segment before it leads to, so a string is a usage of each relation method it names,
//! and what a segment says goes to that method.

use php_index::framework::eloquent::{RelationInfo, relations};
use php_index::framework::keys::KeyKind;
use php_index::{Index, Type};
use php_syntax::TextRange;

use super::keys::KeyString;
use crate::ast::range_of;
use crate::infer::Analyzer;

/// The model a query, a relation or a collection is of: `Builder<User>`, `HasMany<Post, User>`,
/// `Collection<int, User>`, or a model itself.
pub fn model_of(analyzer: &Analyzer<'_>, receiver: &Type) -> Option<String> {
    let index = analyzer.index;
    let receiver = analyzer.receiver_type(receiver);
    for member in receiver.members() {
        let Type::Class { name, args } = member else {
            continue;
        };
        if relations(index, name).is_some() {
            return Some(name.clone());
        }
        for arg in args {
            if let Type::Class { name, .. } = arg {
                if relations(index, name).is_some() {
                    return Some(name.clone());
                }
            }
        }
    }
    None
}

/// A segment of a relation string: the relation it names on the model before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub name: String,
    pub range: TextRange,
    /// The model the segment is a relation of.
    pub model: String,
    pub relation: Option<RelationInfo>,
}

/// The segments of a relation string, as far as each one is known; a segment after one that is
/// not known has no model and is left out.
pub fn segments(index: &Index, key: &KeyString) -> Vec<Segment> {
    if key.kind != KeyKind::Relation {
        return Vec::new();
    }
    let Some(mut model) = key.scope.clone() else {
        return Vec::new();
    };
    // `posts:id,title` selects columns, `posts as published` names a count.
    let name_end = key.value.find([':', ' ']).unwrap_or(key.value.len());
    let start = u32::from(key.range.start());
    let mut out = Vec::new();
    let mut offset = 0usize;
    for name in key.value[..name_end].split('.') {
        let found = relations(index, &model)
            .unwrap_or_default()
            .into_iter()
            .find(|relation| relation.name == name);
        let next = found.as_ref().and_then(|relation| relation.related.clone());
        out.push(Segment {
            name: name.to_string(),
            range: range_of(start + offset as u32, start + (offset + name.len()) as u32),
            model: model.clone(),
            relation: found,
        });
        offset += name.len() + 1;
        match next {
            Some(next) => model = next,
            None => break,
        }
    }
    out
}

/// The segment an offset is in.
pub fn segment_at(index: &Index, key: &KeyString, offset: u32) -> Option<Segment> {
    segments(index, key)
        .into_iter()
        .find(|segment| u32::from(segment.range.start()) <= offset && offset <= u32::from(segment.range.end()))
}
