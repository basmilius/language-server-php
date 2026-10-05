//! The strings that name something a project declares, asked the same way whatever the framework:
//! what can be written there, where a name is declared, whether a name is certainly wrong.

use std::path::PathBuf;

use super::abilities::Abilities;
use super::config::ConfigKeys;
use super::env::EnvNames;
use super::routes::Routes;
use super::symfony::doctrine::Entities;
use super::symfony::events::EventNames;
use super::symfony::routes::SfRoutes;
use super::symfony::services::Services;
use super::symfony::templates::Templates;
use super::symfony::translations::SfTranslations;
use super::translations::Translations;
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
    }
    out
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
