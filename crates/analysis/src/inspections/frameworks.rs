//! Names that a framework looks up in the project and that the project does not declare: a config
//! key, a route, a view, a translation. Only a name the project certainly lacks is reported.

use php_index::framework::keys::{KeyKind, is_missing};

use super::Cx;
use crate::frameworks::keys::keys_in;

pub(super) fn run(cx: &Cx) {
    if !cx.ready || !cx.index.frameworks().any() {
        return;
    }
    let wanted = [
        KeyKind::Config,
        KeyKind::Route,
        KeyKind::View,
        KeyKind::Translation,
        KeyKind::InertiaPage,
        KeyKind::Feature,
        KeyKind::SerializerGroup,
        KeyKind::Workflow,
    ]
    .iter()
    .any(|kind| kind.inspection().is_some_and(|code| cx.on(code)))
        || cx.on("unknown-relation")
        || cx.on("unknown-validation-rule")
        || cx.on("unknown-cast")
        || cx.on("unknown-entity-field")
        || cx.on("unknown-model-key");
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
    if cx.on("unknown-entity-field") {
        unknown_entity_fields(cx);
    }
    let relations = cx.on("unknown-relation")
        && cx.index.frameworks().eloquent
        && !cx
            .index
            .section::<php_index::framework::eloquent::DynamicRelations>()
            .found;
    for key in keys_in(&cx.file) {
        if matches!(
            key.kind,
            KeyKind::ModelColumn | KeyKind::ModelProperty | KeyKind::ModelRelation
        ) {
            if cx.on("unknown-model-key") {
                unknown_model_key(cx, &key);
            }
            continue;
        }
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
            KeyKind::InertiaPage => format!("No page component is named '{}'", key.value),
            KeyKind::Feature => format!("No feature is defined as '{}'", key.value),
            KeyKind::SerializerGroup => format!("No property or method is in the group '{}'", key.value),
            KeyKind::Workflow => format!("No workflow is named '{}'", key.value),
            KeyKind::WorkflowTransition => format!("No workflow has the transition '{}'", key.value),
            KeyKind::WorkflowPlace => format!("No workflow has the place '{}'", key.value),
            _ => format!("The translation '{}' is not in the language files", key.value),
        };
        cx.report(code, key.range, message, super::Fix::None);
    }
}

/// A key of a Raxos model the model does not have under any of its names.
fn unknown_model_key(cx: &Cx, key: &crate::frameworks::keys::KeyString) {
    let Some(model) = &key.scope else {
        return;
    };
    if key.value.is_empty()
        || key.value == "*"
        || !php_index::framework::keys::is_missing_model_key(cx.index, key.kind, model, &key.value)
    {
        return;
    }
    let short = model.rsplit('\\').next().unwrap_or(model);
    let what = match key.kind {
        KeyKind::ModelColumn => "column",
        KeyKind::ModelRelation => "relation",
        _ => "property",
    };
    cx.report(
        "unknown-model-key",
        key.range,
        format!("The model '{short}' has no {what} '{}'", key.value),
        super::Fix::None,
    );
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

/// The fields DQL names that the entity of their alias does not have, nor any entity below it.
fn unknown_entity_fields(cx: &Cx) {
    let entities = cx.index.section::<php_index::framework::symfony::doctrine::Entities>();
    for part in crate::frameworks::dql::parts_in(&cx.file) {
        let crate::frameworks::dql::Part::Field { entity, name, range } = part else {
            continue;
        };
        if name.is_empty() || entities.find(&entity).is_none() {
            continue;
        }
        let ty = php_index::Type::class(entity.clone());
        if cx.index.find_property(&ty, &name).is_some() || super::members::a_subtype_declares(cx, &entity, &name, false)
        {
            continue;
        }
        let short = entity.rsplit('\\').next().unwrap_or(&entity);
        cx.report(
            "unknown-entity-field",
            range,
            format!("The entity '{short}' has no field '{name}'"),
            super::Fix::None,
        );
    }
}
