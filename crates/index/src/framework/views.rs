//! Blade views and components: `view('mail.welcome')` is `resources/views/mail/welcome.blade.php`,
//! and `<x-alert>` is a component class or a template under `resources/views/components`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::Section;
use super::inflect::snake;
use crate::index::{Index, Origin};

const EXTENSIONS: [&str; 4] = [".blade.php", ".php", ".html", ".css"];
const SERVICE_PROVIDER: &str = "Illuminate\\Support\\ServiceProvider";

#[derive(Clone, Debug, PartialEq)]
pub struct ViewFile {
    /// With dots: `mail.welcome`.
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    /// What follows `x-`, with dots for folders: `forms.input`.
    pub tag: String,
    pub path: PathBuf,
    /// The component is a class, and `path` is its file.
    pub class: Option<String>,
}

#[derive(Default)]
pub struct Views {
    pub views: Vec<ViewFile>,
    by_name: HashMap<String, usize>,
    pub components: Vec<Component>,
    /// A provider adds view locations or namespaces the project does not show.
    pub incomplete: bool,
}

impl Views {
    pub fn find(&self, name: &str) -> Option<&ViewFile> {
        self.by_name.get(&normalize(name)).map(|at| &self.views[*at])
    }

    pub fn component(&self, tag: &str) -> Option<&Component> {
        self.components.iter().find(|component| component.tag == tag)
    }

    /// Whether the view is certainly not there. A name with `::` belongs to a package.
    pub fn is_missing(&self, name: &str) -> bool {
        !self.incomplete && !name.contains("::") && !name.is_empty() && self.find(name).is_none()
    }
}

/// `a/b` and `a.b` are the same view.
fn normalize(name: &str) -> String {
    name.replace('/', ".")
}

impl Section for Views {
    fn build(index: &Index) -> Self {
        let mut views = Views::default();
        let root = index.framework_root();
        let dir = root.join("resources").join("views");
        for path in index.files_below(&dir) {
            let Ok(relative) = path.strip_prefix(&dir) else {
                continue;
            };
            let text = relative.to_string_lossy().replace('\\', "/");
            let Some(stem) = EXTENSIONS.iter().find_map(|extension| text.strip_suffix(extension)) else {
                continue;
            };
            let name = stem.replace('/', ".");
            if let Some(component) = name.strip_prefix("components.") {
                let tag = component.strip_suffix(".index").unwrap_or(component);
                views.components.push(Component {
                    tag: tag.to_string(),
                    path: path.clone(),
                    class: None,
                });
            }
            views.by_name.entry(name.clone()).or_insert(views.views.len());
            views.views.push(ViewFile { name, path });
        }
        for class in index.class_names() {
            if class.file.origin != Origin::Project {
                continue;
            }
            let Some(rest) = class.summary.name.strip_prefix("App\\View\\Components\\") else {
                continue;
            };
            let tag = rest
                .split('\\')
                .map(|segment| snake(segment).replace('_', "-"))
                .collect::<Vec<_>>()
                .join(".");
            views.components.push(Component {
                tag,
                path: class.file.path.clone(),
                class: Some(class.summary.name.clone()),
            });
        }
        views.incomplete = adds_locations(index);
        views
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_below(root, path, "resources/views")
            || super::is_below(root, path, "app")
            || super::is_project_php(root, path)
    }
}

fn adds_locations(index: &Index) -> bool {
    index.all_subtypes(SERVICE_PROVIDER).iter().any(|provider| {
        provider.file.origin == Origin::Project
            && index.read_text(&provider.file.path).is_some_and(|text| {
                text.contains("addLocation") || text.contains("replaceNamespace") || text.contains("prependLocation")
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn views_are_named_by_their_folders() {
        let index = project(&[
            ("resources/views/welcome.blade.php", "<h1>hi</h1>"),
            ("resources/views/mail/welcome.blade.php", "x"),
            ("resources/views/components/alert.blade.php", "x"),
            ("resources/views/components/card/index.blade.php", "x"),
            (
                "app/View/Components/Forms/TextInput.php",
                "<?php namespace App\\View\\Components\\Forms; class TextInput {}",
            ),
        ]);
        let views = index.section::<Views>();
        assert!(views.find("welcome").is_some());
        assert!(views.find("mail.welcome").is_some());
        assert!(views.find("mail/welcome").is_some());
        assert!(views.component("alert").is_some());
        assert!(views.component("card").is_some());
        assert_eq!(
            views
                .component("forms.text-input")
                .and_then(|component| component.class.clone())
                .as_deref(),
            Some("App\\View\\Components\\Forms\\TextInput")
        );
        assert!(views.is_missing("nope"));
        assert!(!views.is_missing("welcome"));
        assert!(!views.is_missing("pkg::nope"), "a package view is not ours to judge");
    }
}
