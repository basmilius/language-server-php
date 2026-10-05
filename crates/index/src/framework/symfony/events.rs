//! The events a project can listen to: the constants of the `*Events` classes the frameworks
//! document with `@Event`, and the classes of the events themselves.

use std::path::{Path, PathBuf};

use crate::framework::Section;
use crate::index::{Index, Origin};
use crate::model::Span;

const EVENT: &str = "Symfony\\Contracts\\EventDispatcher\\Event";

#[derive(Clone, Debug, PartialEq)]
pub struct EventName {
    /// What is passed to the dispatcher: `kernel.request` or the class of the event.
    pub name: String,
    /// The constant that holds it, such as `KernelEvents::REQUEST`.
    pub constant: Option<String>,
    /// The class of the event the name stands for.
    pub event_class: Option<String>,
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Default)]
pub struct EventNames {
    pub events: Vec<EventName>,
}

impl EventNames {
    pub fn find(&self, name: &str) -> Option<&EventName> {
        self.events.iter().find(|event| event.name == name)
    }
}

impl Section for EventNames {
    fn build(index: &Index) -> Self {
        let mut found = EventNames::default();
        for class in index.class_names() {
            if class.file.origin == Origin::Stub || !class.summary.name.ends_with("Events") {
                continue;
            }
            let Some(loaded) = class.load() else {
                continue;
            };
            for constant in &loaded.decl.constants {
                let Some(doc) = &constant.doc else {
                    continue;
                };
                let Some(tag) = doc.tags.iter().find(|tag| tag.name == "Event") else {
                    continue;
                };
                let Some(value) = constant
                    .value
                    .as_ref()
                    .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
                else {
                    continue;
                };
                let event_class = tag
                    .text
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .trim_matches('"')
                    .trim_start_matches('\\')
                    .to_string();
                found.events.push(EventName {
                    name: value,
                    constant: Some(format!(
                        "{}::{}",
                        crate::types::short_name(&loaded.decl.name),
                        constant.name
                    )),
                    event_class: (!event_class.is_empty()).then_some(event_class),
                    path: loaded.file.path.clone(),
                    span: constant.name_span,
                });
            }
        }
        for class in index.all_subtypes(EVENT) {
            if class.file.origin == Origin::Project || class.decl.name.contains("\\Event\\") {
                found.events.push(EventName {
                    name: class.decl.name.clone(),
                    constant: None,
                    event_class: Some(class.decl.name.clone()),
                    path: class.file.path.clone(),
                    span: class.decl.name_span,
                });
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_project_php(root, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn finds_the_events_the_frameworks_document() {
        let index = project(&[
            (
                "vendor/symfony/KernelEvents.php",
                "<?php namespace Symfony\\Component\\HttpKernel; final class KernelEvents {\n    /**\n     * The request.\n     *\n     * @Event(\"Symfony\\Component\\HttpKernel\\Event\\RequestEvent\")\n     */\n    public const REQUEST = 'kernel.request';\n    public const NOT_AN_EVENT = 'x';\n}",
            ),
            (
                "vendor/symfony/Event.php",
                "<?php namespace Symfony\\Contracts\\EventDispatcher; class Event {}",
            ),
            (
                "src/Event/OrderPlaced.php",
                "<?php namespace App\\Event; class OrderPlaced extends \\Symfony\\Contracts\\EventDispatcher\\Event {}",
            ),
        ]);
        let events = index.section::<EventNames>();
        let request = events.find("kernel.request").expect("an event");
        assert_eq!(request.constant.as_deref(), Some("KernelEvents::REQUEST"));
        assert_eq!(
            request.event_class.as_deref(),
            Some("Symfony\\Component\\HttpKernel\\Event\\RequestEvent")
        );
        assert!(events.find("x").is_none());
        assert!(events.find("App\\Event\\OrderPlaced").is_some());
    }
}
