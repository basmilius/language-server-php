//! Names that a framework looks up in the project and that the project does not declare: a config
//! key, a route, a view, a translation. Only a name the project certainly lacks is reported.

use php_index::framework::keys::{KeyKind, is_missing};

use super::Cx;
use crate::frameworks::keys::keys_in;

pub(super) fn run(cx: &Cx) {
    if !cx.ready || !cx.index.frameworks().any() {
        return;
    }
    let wanted = [KeyKind::Config, KeyKind::Route, KeyKind::View, KeyKind::Translation]
        .iter()
        .any(|kind| kind.inspection().is_some_and(|code| cx.on(code)))
        || cx.on("unknown-relation")
        || cx.on("unknown-validation-rule")
        || cx.on("unknown-cast");
    if !wanted {
        return;
    }
    if cx.on("unknown-validation-rule") && cx.index.frameworks().laravel {
        for set in crate::frameworks::rules::rule_sets(&cx.file) {
            for part in &set.parts {
                let crate::frameworks::rules::Part::Rule { name, range } = part else {
                    continue;
                };
                let analyzer = cx.file.analyzer(&cx.file.root);
                if crate::frameworks::rules::is_unknown_rule(&analyzer, name) {
                    cx.report(
                        "unknown-validation-rule",
                        *range,
                        format!("The validator has no rule '{name}'"),
                        super::Fix::None,
                    );
                }
            }
        }
    }
    if cx.on("unknown-cast") {
        unknown_casts(cx);
    }
    let relations = cx.on("unknown-relation")
        && cx.index.frameworks().eloquent
        && !cx
            .index
            .section::<php_index::framework::eloquent::DynamicRelations>()
            .found;
    for key in keys_in(&cx.file) {
        if key.kind == KeyKind::Relation {
            if relations {
                unknown_relations(cx, &key);
            }
            continue;
        }
        let Some(code) = key.kind.inspection().filter(|code| cx.on(code)) else {
            continue;
        };
        if key.guarded || key.value.is_empty() || !is_missing(cx.index, key.kind, &key.value) {
            continue;
        }
        let message = match key.kind {
            KeyKind::Config => format!("The config key '{}' is not in the config files", key.value),
            KeyKind::Route => format!("No route is named '{}'", key.value),
            KeyKind::View => format!("The view '{}' does not exist", key.value),
            KeyKind::Template => format!("The template '{}' does not exist", key.value),
            _ => format!("The translation '{}' is not in the language files", key.value),
        };
        cx.report(code, key.range, message, super::Fix::None);
    }
}

/// The segments of a relation string that name no method of their model at all. A method that is
/// there and not read as a relation is given the benefit of the doubt.
fn unknown_relations(cx: &Cx, key: &crate::frameworks::keys::KeyString) {
    for segment in crate::frameworks::relations::segments(cx.index, key) {
        if segment.relation.is_some() || segment.name.is_empty() || segment.name == "*" {
            continue;
        }
        let model = php_index::Type::class(segment.model.clone());
        if cx.index.find_method(&model, &segment.name).is_some() {
            continue;
        }
        // A query of a parent model may run for a child that declares the relation.
        let in_a_child = cx.index.all_subtypes(&segment.model).iter().any(|child| {
            cx.index
                .find_method(&php_index::Type::class(child.decl.name.clone()), &segment.name)
                .is_some()
        });
        if in_a_child {
            continue;
        }
        cx.report(
            "unknown-relation",
            segment.range,
            format!(
                "The model '{}' has no relation '{}'",
                crate::short(&segment.model),
                segment.name
            ),
            super::Fix::None,
        );
    }
}

/// The casts written as strings that a model does not know, by name or as a class. A string with a
/// backslash names a class that may be missing for reasons of its own, so it is left to that.
fn unknown_casts(cx: &Cx) {
    let Some(primitives) = php_index::framework::eloquent::primitive_casts(cx.index) else {
        return;
    };
    for (cast, range) in crate::frameworks::casts::cast_values(&cx.file) {
        if cast.is_empty()
            || cast.contains('\\')
            || php_index::framework::eloquent::is_known_cast(cx.index, &primitives, &cast)
        {
            continue;
        }
        cx.report(
            "unknown-cast",
            range,
            format!("A model has no cast '{cast}'"),
            super::Fix::None,
        );
    }
}
