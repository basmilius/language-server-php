//! Inertia: the page components a response renders by name (`Inertia::render('Users/Index')`), the
//! files of the JavaScript pages folder that Inertia's own testing config points at by default.

use std::path::{Path, PathBuf};

use super::Section;
use crate::index::Index;

/// The folders a project keeps its pages in, as the starter kits name them.
const FOLDERS: [&str; 3] = ["resources/js/Pages", "resources/js/pages", "resources/ts/Pages"];
const EXTENSIONS: [&str; 6] = ["vue", "tsx", "jsx", "svelte", "ts", "js"];

#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    /// With slashes and no extension: `Users/Index`.
    pub name: String,
    pub path: PathBuf,
}

#[derive(Default)]
pub struct InertiaPages {
    pub pages: Vec<Page>,
    /// A pages folder is there, so a name it lacks is certainly missing.
    pub found_folder: bool,
}

impl InertiaPages {
    pub fn find(&self, name: &str) -> Option<&Page> {
        let name = name.trim_matches('/').replace('.', "/");
        self.pages.iter().find(|page| page.name == name)
    }

    pub fn is_missing(&self, name: &str) -> bool {
        self.found_folder && !name.is_empty() && !name.contains("::") && self.find(name).is_none()
    }
}

impl Section for InertiaPages {
    fn build(index: &Index) -> Self {
        let mut found = InertiaPages::default();
        if index.class("Inertia\\Inertia").is_none() {
            return found;
        }
        let root = index.framework_root();
        for folder in FOLDERS {
            let dir = root.join(folder);
            let files = index.files_below(&dir);
            if files.is_empty() {
                continue;
            }
            found.found_folder = true;
            for path in files {
                let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
                    continue;
                };
                if !EXTENSIONS.contains(&extension) {
                    continue;
                }
                let Ok(relative) = path.strip_prefix(&dir) else {
                    continue;
                };
                let name = relative.with_extension("").to_string_lossy().replace('\\', "/");
                if found.find(&name).is_none() {
                    found.pages.push(Page { name, path });
                }
            }
        }
        found.pages.sort_by(|left, right| left.name.cmp(&right.name));
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        FOLDERS.iter().any(|folder| path.starts_with(root.join(folder)))
    }
}
