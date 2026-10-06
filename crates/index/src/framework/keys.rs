//! The strings that name something a project declares, asked the same way whatever the framework:
//! what can be written there, where a name is declared, whether a name is certainly wrong.

use std::path::PathBuf;

use super::abilities::Abilities;
use super::config::ConfigKeys;
use super::env::EnvNames;
use super::layouts::{Layouts, attribute_key, component_template, kebab, props_of, slots_of};
use super::routes::Routes;
use super::symfony::doctrine::Entities;
use super::symfony::events::EventNames;
use super::symfony::routes::SfRoutes;
use super::symfony::services::Services;
use super::symfony::templates::Templates;
use super::symfony::translations::SfTranslations;
use super::translations::Translations;
use super::twig::TwigShapes;
use super::views::Views;
use crate::index::Index;
use crate::model::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum KeyKind {
    Config,
    Route,
    View,
    Translation,
    Env,
    Ability,
    /// A field of the form request the call is made on.
    Field,
    /// The name of a Blade component, after `<x-`.
    Component,
    /// The id of a service of the container.
    Service,
    /// The name of a container parameter.
    Parameter,
    /// A Twig template.
    Template,
    /// The name of an event to listen to or dispatch.
    Event,
    /// A field of the entity a repository serves.
    EntityField,
    /// A section a layout yields and a child fills.
    Section,
    /// A stack a layout renders and templates push to.
    Stack,
    /// A slot of the component the scope names.
    Slot,
    /// An attribute of the component the scope names, which fills one of its props.
    Attribute,
    /// A block of a Twig template; the scope is the template it is written in, whose parents
    /// declare it.
    Block,
    /// A relation of the Eloquent model the scope names.
    Relation,
    /// A table the migrations create.
    Table,
    /// A column of the table the scope names.
    Column,
}

impl KeyKind {
    /// The kind an overlay marker names.
    pub fn parse(word: &str) -> Option<KeyKind> {
        Some(match word {
            "config" => KeyKind::Config,
            "route" => KeyKind::Route,
            "view" => KeyKind::View,
            "translation" => KeyKind::Translation,
            "env" => KeyKind::Env,
            "ability" => KeyKind::Ability,
            "field" => KeyKind::Field,
            "service" => KeyKind::Service,
            "parameter" => KeyKind::Parameter,
            "template" => KeyKind::Template,
            "event" => KeyKind::Event,
            "entity-field" => KeyKind::EntityField,
            "section" => KeyKind::Section,
            "stack" => KeyKind::Stack,
            "relation" => KeyKind::Relation,
            "column" => KeyKind::Column,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            KeyKind::Config => "config key",
            KeyKind::Route => "route",
            KeyKind::View => "view",
            KeyKind::Translation => "translation",
            KeyKind::Env => "environment variable",
            KeyKind::Ability => "ability",
            KeyKind::Field => "validated field",
            KeyKind::Component => "component",
            KeyKind::Service => "service",
            KeyKind::Parameter => "parameter",
            KeyKind::Template => "template",
            KeyKind::Event => "event",
            KeyKind::EntityField => "entity field",
            KeyKind::Section => "section",
            KeyKind::Stack => "stack",
            KeyKind::Slot => "slot",
            KeyKind::Attribute => "component attribute",
            KeyKind::Block => "block",
            KeyKind::Relation => "relation",
            KeyKind::Table => "table",
            KeyKind::Column => "column",
        }
    }

    /// The inspection that reports a name of this kind that does not exist.
    pub fn inspection(self) -> Option<&'static str> {
        match self {
            KeyKind::Config => Some("unknown-config-key"),
            KeyKind::Route => Some("unknown-route"),
            KeyKind::View => Some("unknown-view"),
            KeyKind::Translation => Some("unknown-translation"),
            KeyKind::Template => Some("unknown-template"),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub key: String,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub path: PathBuf,
    pub span: Span,
    pub detail: String,
}

fn candidate(key: &str, detail: impl Into<Option<String>>) -> Candidate {
    Candidate {
        key: key.to_string(),
        detail: detail.into(),
    }
}

fn definition(path: &std::path::Path, span: Span, detail: impl Into<String>) -> Definition {
    Definition {
        path: path.to_path_buf(),
        span,
        detail: detail.into(),
    }
}

/// Every name that can be written, to complete from.
pub fn candidates(index: &Index, kind: KeyKind, scope: Option<&str>) -> Vec<Candidate> {
    let frameworks = index.frameworks();
    let mut out = Vec::new();
    match kind {
        KeyKind::Field => {
            if let Some(class) = scope {
                out.extend(
                    super::validation::fields_of(index, class)
                        .iter()
                        .map(|field| candidate(&field.name, None)),
                );
            }
        }
        KeyKind::EntityField => {
            if let Some(entity) = scope {
                if let Some(info) = index.section::<Entities>().find(entity) {
                    out.extend(info.fields.iter().map(|field| candidate(&field.name, None)));
                }
            }
        }
        KeyKind::Config => out.extend(
            index
                .section::<ConfigKeys>()
                .keys()
                .map(|entry| candidate(&entry.key, entry.value.clone())),
        ),
        KeyKind::Component => out.extend(
            index
                .section::<Views>()
                .components
                .iter()
                .map(|component| candidate(&component.tag, component.class.clone())),
        ),
        KeyKind::Route => {
            if frameworks.laravel {
                out.extend(
                    index
                        .section::<Routes>()
                        .names
                        .iter()
                        .map(|route| candidate(&route.name, None)),
                );
            }
            if frameworks.symfony {
                out.extend(
                    index
                        .section::<SfRoutes>()
                        .routes
                        .iter()
                        .map(|route| candidate(&route.name, route.path.clone())),
                );
            }
        }
        KeyKind::View => out.extend(
            index
                .section::<Views>()
                .views
                .iter()
                .map(|view| candidate(&view.name, None)),
        ),
        KeyKind::Template => out.extend(
            index
                .section::<Templates>()
                .templates
                .iter()
                .map(|template| candidate(&template.name, None)),
        ),
        KeyKind::Translation => {
            let mut seen = std::collections::HashSet::new();
            if frameworks.laravel {
                let translations = index.section::<Translations>();
                out.extend(
                    translations
                        .keys()
                        .filter(|entry| !entry.group || entry.value.is_some())
                        .filter(|entry| seen.insert(entry.key.clone()))
                        .map(|entry| candidate(&entry.key, entry.value.clone())),
                );
            }
            if frameworks.symfony {
                let translations = index.section::<SfTranslations>();
                out.extend(
                    translations
                        .entries
                        .iter()
                        .filter(|entry| seen.insert(entry.key.clone()))
                        .map(|entry| candidate(&entry.key, entry.value.clone().or_else(|| Some(entry.domain.clone())))),
                );
            }
        }
        KeyKind::Env => {
            let env = index.section::<EnvNames>();
            let mut seen = std::collections::HashSet::new();
            out.extend(
                env.names
                    .iter()
                    .filter(|entry| seen.insert(entry.name.clone()))
                    .map(|entry| candidate(&entry.name, None)),
            );
        }
        KeyKind::Ability => {
            let abilities = index.section::<Abilities>();
            let mut seen = std::collections::HashSet::new();
            out.extend(
                abilities
                    .abilities
                    .iter()
                    .filter(|ability| seen.insert(ability.name.clone()))
                    .map(|ability| candidate(&ability.name, ability.policy.clone())),
            );
        }
        KeyKind::Service => {
            let services = index.section::<Services>();
            out.extend(services.services.iter().map(|service| {
                candidate(
                    &service.id,
                    service
                        .class
                        .clone()
                        .or_else(|| service.alias_of.as_ref().map(|target| format!("alias of {target}"))),
                )
            }));
        }
        KeyKind::Parameter => {
            let services = index.section::<Services>();
            out.extend(
                services
                    .parameters
                    .iter()
                    .map(|parameter| candidate(&parameter.name, parameter.value.clone())),
            );
        }
        KeyKind::Event => {
            let events = index.section::<EventNames>();
            out.extend(events.events.iter().map(|event| {
                candidate(
                    &event.name,
                    event.constant.clone().or_else(|| event.event_class.clone()),
                )
            }));
        }
        KeyKind::Section | KeyKind::Stack => {
            let layouts = index.section::<Layouts>();
            let placed = if kind == KeyKind::Section {
                &layouts.sections
            } else {
                &layouts.stacks
            };
            let mut seen = std::collections::HashSet::new();
            out.extend(
                placed
                    .iter()
                    .filter(|placed| seen.insert(placed.name.clone()))
                    .map(|placed| candidate(&placed.name, None)),
            );
        }
        KeyKind::Slot => {
            if let Some(text) = scope.and_then(|tag| component_text(index, tag)) {
                out.extend(slots_of(&text).iter().map(|(name, _)| candidate(name, None)));
            }
        }
        KeyKind::Table => {
            let tables = index.section::<super::migrations::Tables>();
            let mut names: Vec<&String> = tables.names().collect();
            names.sort();
            out.extend(names.into_iter().map(|name| candidate(name, None)));
        }
        KeyKind::Column => {
            let tables = index.section::<super::migrations::Tables>();
            if let Some(table) = scope.and_then(|table| tables.table(table)) {
                out.extend(table.columns.iter().map(|column| candidate(&column.name, None)));
            }
        }
        KeyKind::Relation => {
            if let Some(found) = scope.and_then(|model| super::eloquent::relations(index, model)) {
                let mut seen = std::collections::HashSet::new();
                out.extend(
                    found
                        .into_iter()
                        .filter(|relation| seen.insert(relation.name.clone()))
                        .map(|relation| {
                            candidate(
                                &relation.name,
                                relation
                                    .related
                                    .map(|related| crate::types::short_name(&related).to_string()),
                            )
                        }),
                );
            }
        }
        KeyKind::Block => {
            let mut seen = std::collections::HashSet::new();
            for (_, block, _) in twig_blocks(index, scope) {
                if seen.insert(block.clone()) {
                    out.push(candidate(&block, None));
                }
            }
        }
        KeyKind::Attribute => {
            if let Some(tag) = scope {
                out.extend(
                    component_props(index, tag)
                        .into_iter()
                        .map(|prop| candidate(&kebab(&prop.name), prop.detail)),
                );
            }
        }
    }
    out
}

/// The blocks a template can fill: those of its parents, nearest first. Without a template, every
/// block of every template.
fn twig_blocks(index: &Index, template: Option<&str>) -> Vec<(std::path::PathBuf, String, Span)> {
    let shapes = index.section::<TwigShapes>();
    let templates = index.section::<Templates>();
    let Some(name) = template else {
        return shapes
            .all()
            .flat_map(|(path, shape)| {
                shape
                    .blocks
                    .iter()
                    .map(move |(block, span)| (path.clone(), block.clone(), *span))
            })
            .collect();
    };
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut parent = templates
        .find(name)
        .and_then(|found| shapes.of(&found.path))
        .and_then(|shape| shape.extends.clone());
    while let Some(next) = parent.filter(|next| seen.insert(next.clone())) {
        let Some(found) = templates.find(&next) else {
            break;
        };
        let Some(shape) = shapes.of(&found.path) else {
            break;
        };
        out.extend(
            shape
                .blocks
                .iter()
                .map(|(block, span)| (found.path.clone(), block.clone(), *span)),
        );
        parent = shape.extends.clone();
    }
    out
}

/// A prop of a component: an entry of `@props`, or a parameter of the constructor of its class.
struct Prop {
    name: String,
    path: std::path::PathBuf,
    span: Span,
    detail: Option<String>,
}

fn component_text(index: &Index, tag: &str) -> Option<String> {
    let path = component_template(index, tag)?;
    index.read_text(&path).map(|text| text.to_string())
}

fn component_props(index: &Index, tag: &str) -> Vec<Prop> {
    let mut out = Vec::new();
    let views = index.section::<Views>();
    if let Some(class) = views
        .components
        .iter()
        .find(|component| component.tag == tag && component.class.is_some())
        .and_then(|component| component.class.as_deref())
        .and_then(|class| index.class(class))
    {
        if let Some(constructor) = class.decl.method("__construct") {
            out.extend(constructor.callable.params.iter().map(|param| Prop {
                name: param.name.clone(),
                path: class.file.path.clone(),
                span: param.span,
                detail: param.ty.as_ref().map(|ty| ty.display(true)),
            }));
        }
    }
    if let Some(path) = component_template(index, tag) {
        if let Some(text) = index.read_text(&path) {
            out.extend(props_of(&text).into_iter().map(|(name, span)| Prop {
                name,
                path: path.clone(),
                span,
                detail: None,
            }));
        }
    }
    out
}

/// Where a name is declared.
pub fn definitions(index: &Index, kind: KeyKind, key: &str, scope: Option<&str>) -> Vec<Definition> {
    let frameworks = index.frameworks();
    let mut out = Vec::new();
    match kind {
        KeyKind::Field => {
            if let Some(class) = scope {
                out.extend(
                    super::validation::fields_of(index, class)
                        .into_iter()
                        .filter(|field| field.name == key)
                        .map(|field| definition(&field.path, field.span, "")),
                );
            }
        }
        KeyKind::EntityField => {
            if let Some(class) = scope.and_then(|entity| index.class(entity)) {
                if let Some(property) = class.decl.property(key) {
                    out.push(definition(&class.file.path, property.name_span, ""));
                }
            }
        }
        KeyKind::Config => out.extend(
            index
                .section::<ConfigKeys>()
                .find(key)
                .map(|entry| definition(&entry.path, entry.span, entry.value.clone().unwrap_or_default())),
        ),
        KeyKind::Component => out.extend(index.section::<Views>().component(key).map(|component| {
            definition(
                &component.path,
                Span::default(),
                component.class.clone().unwrap_or_default(),
            )
        })),
        KeyKind::Route => {
            if frameworks.laravel {
                out.extend(
                    index
                        .section::<Routes>()
                        .find(key)
                        .map(|route| definition(&route.path, route.span, "")),
                );
            }
            if frameworks.symfony {
                out.extend(
                    index
                        .section::<SfRoutes>()
                        .find(key)
                        .map(|route| definition(&route.file, route.span, route.path.clone().unwrap_or_default())),
                );
            }
        }
        KeyKind::View => out.extend(
            index
                .section::<Views>()
                .find(key)
                .map(|view| definition(&view.path, Span::default(), "")),
        ),
        KeyKind::Template => out.extend(
            index
                .section::<Templates>()
                .find(key)
                .map(|template| definition(&template.path, Span::default(), "")),
        ),
        KeyKind::Translation => {
            if frameworks.laravel {
                out.extend(index.section::<Translations>().find(key).into_iter().map(|entry| {
                    let detail = match (&entry.locale, &entry.value) {
                        (Some(locale), Some(value)) => format!("{locale}: {value}"),
                        (Some(locale), None) => locale.clone(),
                        _ => String::new(),
                    };
                    definition(&entry.path, entry.span, detail)
                }));
            }
            if frameworks.symfony {
                out.extend(
                    index
                        .section::<SfTranslations>()
                        .find(key, None)
                        .into_iter()
                        .map(|entry| {
                            let detail = match &entry.value {
                                Some(value) => format!("{}: {value}", entry.locale),
                                None => entry.locale.clone(),
                            };
                            definition(&entry.path, entry.span, detail)
                        }),
                );
            }
        }
        KeyKind::Env => out.extend(
            index
                .section::<EnvNames>()
                .find_all(key)
                .into_iter()
                .map(|entry| definition(&entry.path, entry.span, "")),
        ),
        KeyKind::Ability => out.extend(
            index
                .section::<Abilities>()
                .find_all(key)
                .into_iter()
                .map(|ability| definition(&ability.path, ability.span, ability.policy.clone().unwrap_or_default())),
        ),
        KeyKind::Service => {
            let services = index.section::<Services>();
            if let Some(service) = services.find(key) {
                out.push(definition(
                    &service.path,
                    service.span,
                    service.class.clone().unwrap_or_default(),
                ));
            } else if let Some(class) = index.class(key) {
                out.push(definition(&class.file.path, class.decl.name_span, ""));
            }
        }
        KeyKind::Parameter => out.extend(index.section::<Services>().parameter(key).map(|parameter| {
            definition(
                &parameter.path,
                parameter.span,
                parameter.value.clone().unwrap_or_default(),
            )
        })),
        KeyKind::Event => out.extend(
            index
                .section::<EventNames>()
                .find(key)
                .map(|event| definition(&event.path, event.span, event.event_class.clone().unwrap_or_default())),
        ),
        KeyKind::Section => out.extend(
            index
                .section::<Layouts>()
                .sections_named(key)
                .map(|placed| definition(&placed.path, placed.span, "")),
        ),
        KeyKind::Stack => out.extend(
            index
                .section::<Layouts>()
                .stacks_named(key)
                .map(|placed| definition(&placed.path, placed.span, "")),
        ),
        KeyKind::Slot => {
            if let Some(tag) = scope {
                if let (Some(path), Some(text)) = (component_template(index, tag), component_text(index, tag)) {
                    out.extend(
                        slots_of(&text)
                            .into_iter()
                            .filter(|(name, _)| name == key)
                            .map(|(_, span)| definition(&path, span, "")),
                    );
                }
            }
        }
        KeyKind::Table => {
            let tables = index.section::<super::migrations::Tables>();
            // The first column that names itself, in the migration that creates the table.
            if let Some(first) = tables
                .table(key)
                .and_then(|table| table.columns.iter().find(|column| column.span.end > column.span.start))
            {
                out.push(definition(&first.path, first.span, ""));
            }
        }
        KeyKind::Column => {
            let tables = index.section::<super::migrations::Tables>();
            if let Some(column) = scope
                .and_then(|table| tables.table(table))
                .and_then(|table| table.column(key))
            {
                out.push(definition(&column.path, column.span, ""));
            }
        }
        KeyKind::Relation => {
            if let Some(found) = scope.and_then(|model| super::eloquent::relations(index, model)) {
                out.extend(
                    found
                        .into_iter()
                        .filter(|relation| relation.name == key)
                        .map(|relation| definition(&relation.path, relation.name_span, "")),
                );
            }
        }
        KeyKind::Block => out.extend(
            twig_blocks(index, scope)
                .into_iter()
                .filter(|(_, block, _)| block == key)
                .map(|(path, _, span)| definition(&path, span, "")),
        ),
        KeyKind::Attribute => {
            if let Some(tag) = scope {
                let wanted = attribute_key(key);
                out.extend(
                    component_props(index, tag)
                        .into_iter()
                        .filter(|prop| attribute_key(&prop.name) == wanted)
                        .map(|prop| definition(&prop.path, prop.span, prop.detail.unwrap_or_default())),
                );
            }
        }
    }
    out
}

/// The name a file declares at an offset: a route's `->name()`, a key of a config or translation
/// file, a service or parameter of the PHP configuration.
pub fn declared_at(index: &Index, path: &std::path::Path, offset: u32) -> Option<(KeyKind, String, Span)> {
    declared_where(index, |_, definition| {
        definition.path == path
            && definition.span.start < definition.span.end
            && definition.span.start <= offset
            && offset <= definition.span.end
    })
}

/// The column a migration adds at an offset, with its table.
pub fn column_declared_at(index: &Index, path: &std::path::Path, offset: u32) -> Option<(String, String, Span)> {
    let tables = index.section::<super::migrations::Tables>();
    tables.names().find_map(|table| {
        tables.table(table)?.columns.iter().find_map(|column| {
            (column.path == path && column.span.start <= offset && offset <= column.span.end)
                .then(|| (table.clone(), column.name.clone(), column.span))
        })
    })
}

/// The name an attribute of a declaration gives it, when the catalog keeps the name by the name of
/// the declaration: the `name:` of `#[Route]` on a controller method, which may carry the prefix of
/// the class.
pub fn declared_by(index: &Index, path: &std::path::Path, owner: Span, written: &str) -> Option<(KeyKind, String)> {
    declared_where(index, |key, definition| {
        definition.path == path && definition.span == owner && key.ends_with(written)
    })
    .map(|(kind, key, _)| (kind, key))
}

fn declared_where(index: &Index, wanted: impl Fn(&str, &Definition) -> bool) -> Option<(KeyKind, String, Span)> {
    const DECLARED: [KeyKind; 8] = [
        KeyKind::Route,
        KeyKind::Config,
        KeyKind::Translation,
        KeyKind::Service,
        KeyKind::Parameter,
        KeyKind::Event,
        KeyKind::Ability,
        KeyKind::Env,
    ];
    for kind in DECLARED {
        for candidate in candidates(index, kind, None) {
            let found = definitions(index, kind, &candidate.key, None)
                .into_iter()
                .find(|definition| wanted(&candidate.key, definition));
            if let Some(definition) = found {
                return Some((kind, candidate.key, definition.span));
            }
        }
    }
    None
}

/// Whether a name is certainly wrong: the project declares everything of its kind that it can,
/// and the name is not among it.
pub fn is_missing(index: &Index, kind: KeyKind, key: &str) -> bool {
    let frameworks = index.frameworks();
    match kind {
        KeyKind::Config => frameworks.laravel && index.section::<ConfigKeys>().is_missing(index, key),
        KeyKind::Route => {
            let laravel = !frameworks.laravel || index.section::<Routes>().is_missing(index, key);
            let symfony = !frameworks.symfony || index.section::<SfRoutes>().is_missing(key);
            (frameworks.laravel || frameworks.symfony) && laravel && symfony
        }
        KeyKind::View => frameworks.laravel && index.section::<Views>().is_missing(key),
        KeyKind::Template => frameworks.symfony && index.section::<Templates>().is_missing(key),
        KeyKind::Translation => frameworks.laravel && index.section::<Translations>().is_missing(key),
        _ => false,
    }
}
