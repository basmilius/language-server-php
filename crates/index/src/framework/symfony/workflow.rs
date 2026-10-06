//! Symfony Workflow: the workflows and state machines `framework.workflows` configures, with their
//! places and transitions, read from the YAML of `config/`. A transition is a key of `transitions:`
//! or the `name:` of an item of its list, a place an item or a key of `places:`.

use std::path::{Path, PathBuf};

use super::{config_files, yaml_of};
use crate::framework::Section;
use crate::framework::yaml::Node;
use crate::index::Index;
use crate::model::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkflowPart {
    Workflow,
    Place,
    Transition,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Declared {
    pub part: WorkflowPart,
    pub workflow: String,
    pub name: String,
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Default)]
pub struct Workflows {
    pub declared: Vec<Declared>,
    /// A PHP file of `config/` configures workflows too, which is not read.
    pub incomplete: bool,
}

impl Workflows {
    pub fn named(&self, part: WorkflowPart, name: &str) -> impl Iterator<Item = &Declared> {
        let name = name.to_string();
        self.declared
            .iter()
            .filter(move |declared| declared.part == part && declared.name == name)
    }

    pub fn names(&self, part: WorkflowPart) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .declared
            .iter()
            .filter(|declared| declared.part == part)
            .map(|declared| declared.name.as_str())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn is_missing(&self, part: WorkflowPart, name: &str) -> bool {
        !self.incomplete
            && self
                .declared
                .iter()
                .any(|declared| declared.part == WorkflowPart::Workflow)
            && self.named(part, name).next().is_none()
    }
}

impl Section for Workflows {
    fn build(index: &Index) -> Self {
        let mut found = Workflows::default();
        if !index.frameworks().symfony {
            return found;
        }
        for path in config_files(index, &["php"]) {
            if index
                .read_text(&path)
                .is_some_and(|text| text.contains("workflows") || text.contains("->workflows("))
            {
                found.incomplete = true;
            }
        }
        for path in config_files(index, &["yaml", "yml"]) {
            let Some((_, node)) = yaml_of(index, &path) else {
                continue;
            };
            let roots = [
                node.get("framework"),
                node.get("when@dev").and_then(|when| when.get("framework")),
            ];
            for framework in roots.into_iter().flatten() {
                let Some(workflows) = framework.get("workflows") else {
                    continue;
                };
                for (workflow, span, config) in workflows.entries() {
                    found.read_workflow(&path, workflow, span, config);
                }
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        path.starts_with(root.join("config"))
    }
}

impl Workflows {
    fn read_workflow(&mut self, path: &Path, workflow: &str, span: Span, config: &Node) {
        let mut push = |part, name: &str, span: Span| {
            self.declared.push(Declared {
                part,
                workflow: workflow.to_string(),
                name: name.to_string(),
                path: path.to_path_buf(),
                span,
            });
        };
        push(WorkflowPart::Workflow, workflow, span);
        if let Some(places) = config.get("places") {
            for item in places.items() {
                if let (Some(name), Some(span)) = (item.as_str(), item.span()) {
                    push(WorkflowPart::Place, name, span);
                }
            }
            for (name, span, _) in places.entries() {
                push(WorkflowPart::Place, name, span);
            }
        }
        if let Some(transitions) = config.get("transitions") {
            for (name, span, _) in transitions.entries() {
                push(WorkflowPart::Transition, name, span);
            }
            for item in transitions.items() {
                if let Some(name) = item.get("name") {
                    if let (Some(text), Some(span)) = (name.as_str(), name.span()) {
                        push(WorkflowPart::Transition, text, span);
                    }
                }
            }
        }
    }
}
