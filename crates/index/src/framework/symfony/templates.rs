//! Twig templates: `$this->render('blog/index.html.twig')` is `templates/blog/index.html.twig`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::framework::Section;
use crate::index::Index;

#[derive(Clone, Debug, PartialEq)]
pub struct Template {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct Templates {
    pub templates: Vec<Template>,
    by_name: HashMap<String, usize>,
    /// The configuration names other folders than `templates/`, which are not read.
    pub incomplete: bool,
}

impl Templates {
    pub fn find(&self, name: &str) -> Option<&Template> {
        self.by_name.get(name).map(|at| &self.templates[*at])
    }

    /// Whether the template is certainly not there. A name that starts with `@` belongs to a bundle
    /// or to a path the configuration added.
    pub fn is_missing(&self, name: &str) -> bool {
        !self.incomplete && !name.is_empty() && !name.starts_with(['@', '!']) && self.find(name).is_none()
    }
}

impl Section for Templates {
    fn build(index: &Index) -> Self {
        let mut found = Templates::default();
        let dir = index.framework_root().join("templates");
        for path in index.files_below(&dir) {
            if path.extension().is_none_or(|ext| ext != "twig") {
                continue;
            }
            let Ok(relative) = path.strip_prefix(&dir) else {
                continue;
            };
            let name = relative.to_string_lossy().replace('\\', "/");
            found.by_name.insert(name.clone(), found.templates.len());
            found.templates.push(Template { name, path });
        }
        let config = index.framework_root().join("config").join("packages").join("twig.yaml");
        if let Some(text) = index.read_text(&config) {
            found.incomplete = text.lines().any(|line| {
                let trimmed = line.trim_start();
                trimmed.starts_with("paths:")
                    || trimmed.starts_with("default_path:") && !trimmed.contains("%kernel.project_dir%/templates")
            });
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_below(root, path, "templates")
            || path == root.join("config").join("packages").join("twig.yaml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn templates_are_named_by_their_path() {
        let index = project(&[
            ("templates/base.html.twig", "x"),
            ("templates/blog/index.html.twig", "x"),
            ("templates/notes.txt", "x"),
        ]);
        let templates = index.section::<Templates>();
        assert!(templates.find("blog/index.html.twig").is_some());
        assert!(templates.is_missing("blog/other.html.twig"));
        assert!(!templates.is_missing("@Foo/x.html.twig"));
        assert!(templates.find("notes.txt").is_none());
        let configured = project(&[(
            "config/packages/twig.yaml",
            "twig:\n    paths:\n        '%kernel.project_dir%/mails': mails\n",
        )]);
        assert!(!configured.section::<Templates>().is_missing("x.html.twig"));
    }
}
