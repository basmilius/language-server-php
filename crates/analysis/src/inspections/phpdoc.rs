//! Doc comments of functions and methods that disagree with the code under them: a `@param` for a
//! parameter that is not there, and a type that contradicts the one declared.

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange, TextSize};

use super::types::{Kinds, kinds_of};
use super::{Cx, Fix};
use crate::ast::{child_of, first_token, has_token, tokens};
use crate::doc_refs::{DocItemKind, doc_items};

pub(super) fn run(cx: &Cx) {
    if !(cx.on("phpdoc-unknown-parameter") || cx.on("phpdoc-type-mismatch")) {
        return;
    }
    for node in &cx.nodes {
        if matches!(node.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION) {
            check_doc(cx, node);
        }
    }
}

/// A `@param` or `@return` of a doc comment, with the first variable after it.
struct Tag {
    name: String,
    tag_range: TextRange,
    variable: Option<(String, TextRange)>,
}

fn doc_tags(doc: &php_syntax::SyntaxToken) -> Vec<Tag> {
    let base = u32::from(doc.text_range().start());
    let mut tags: Vec<Tag> = Vec::new();
    for item in doc_items(doc.text(), base) {
        let range = TextRange::new(TextSize::from(item.start), TextSize::from(item.end));
        match item.kind {
            DocItemKind::Tag(name) => tags.push(Tag {
                name,
                tag_range: range,
                variable: None,
            }),
            DocItemKind::Variable(name) => {
                if let Some(tag) = tags.last_mut().filter(|tag| tag.variable.is_none()) {
                    tag.variable = Some((name, range));
                }
            }
            _ => {}
        }
    }
    tags
}

fn check_doc(cx: &Cx, function: &SyntaxNode) {
    let Some(doc) = tokens(function).find(|token| token.kind() == DOC_COMMENT) else {
        return;
    };
    let tags = doc_tags(&doc);
    if tags.is_empty() {
        return;
    }
    let names: Vec<String> = child_of(function, PARAMETER_LIST)
        .map(|list| {
            list.children()
                .filter(|child| child.kind() == PARAMETER)
                .filter_map(|parameter| first_token(&parameter, VARIABLE))
                .map(|token| token.text().trim_start_matches('$').to_string())
                .collect()
        })
        .unwrap_or_default();
    let analyzer = cx.file.analyzer(function);
    let callable = super::types::callable_of(&analyzer, function);
    if cx.on("phpdoc-unknown-parameter") && !callable.reads_all_arguments {
        for tag in tags.iter().filter(|tag| tag.name == "param") {
            let Some((variable, range)) = &tag.variable else {
                continue;
            };
            if names.contains(variable) {
                continue;
            }
            cx.report(
                "phpdoc-unknown-parameter",
                *range,
                format!("The function has no parameter '${variable}'"),
                Fix::RemoveDocLine {
                    range: TextRange::new(tag.tag_range.start(), line_content_end(cx, range.end())),
                },
            );
        }
    }
    if cx.on("phpdoc-type-mismatch") {
        check_types(cx, function, &callable, &tags);
    }
}

/// The end of what a tag says: the end of its line, or the `*/` that closes a one-line comment.
fn line_content_end(cx: &Cx, from: TextSize) -> TextSize {
    let rest = &cx.text[usize::from(from)..];
    let line = rest.find(['\n', '\r']).unwrap_or(rest.len());
    let stop = rest[..line].find("*/").unwrap_or(line);
    from + TextSize::from(rest[..stop].trim_end().len() as u32)
}

fn check_types(cx: &Cx, function: &SyntaxNode, callable: &php_index::Callable, tags: &[Tag]) {
    let level = cx.index.level;
    for tag in tags.iter().filter(|tag| tag.name == "param") {
        let Some((variable, range)) = &tag.variable else {
            continue;
        };
        let Some(param) = callable.params.iter().find(|param| &param.name == variable) else {
            continue;
        };
        let (Some(documented), Some(declared)) = (param.doc_ty.as_ref(), param.native_type(level)) else {
            continue;
        };
        if contradicts(cx, documented, declared) {
            cx.report(
                "phpdoc-type-mismatch",
                *range,
                format!(
                    "The documented type '{}' contradicts the declared type '{}'",
                    documented.display(true),
                    declared.display(true)
                ),
                Fix::None,
            );
        }
    }
    let (Some(documented), Some(declared)) = (callable.doc_ret.as_ref(), callable.native_return(level)) else {
        return;
    };
    let Some(tag) = tags.iter().find(|tag| tag.name == "return") else {
        return;
    };
    if !has_token(function, FUNCTION_KW) && function.kind() != METHOD_DECLARATION {
        return;
    }
    if contradicts(cx, documented, declared) {
        cx.report(
            "phpdoc-type-mismatch",
            tag.tag_range,
            format!(
                "The documented return type '{}' contradicts the declared type '{}'",
                documented.display(true),
                declared.display(true)
            ),
            Fix::None,
        );
    }
}

/// Whether a documented type and a declared one cannot describe the same value.
fn contradicts(cx: &Cx, documented: &Type, declared: &Type) -> bool {
    if matches!(declared, Type::Void | Type::Never) || matches!(documented, Type::Void | Type::Never) {
        return false;
    }
    // A name that is no class here may be a type alias of the doc comment.
    let known = |ty: &Type| ty.class_names().iter().all(|name| cx.index.class(name).is_some());
    if !known(documented) || !known(declared) {
        return false;
    }
    let (Some(doc_kinds), Some(mut native_kinds)) = (kinds_of(documented), kinds_of(declared)) else {
        return false;
    };
    // An `int` is accepted where `float` is declared.
    if native_kinds.contains(Kinds::FLOAT) {
        native_kinds = native_kinds.union(Kinds::INT);
    }
    if !doc_kinds.intersects(native_kinds) {
        return true;
    }
    cx.classes_cannot_overlap(documented, declared)
}
