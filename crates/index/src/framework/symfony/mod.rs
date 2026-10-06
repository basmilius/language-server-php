//! Symfony: the services, parameters, routes, templates, translations and events that strings name,
//! and Doctrine's entities and repositories.

pub mod doctrine;
pub mod events;
pub mod routes;
pub mod serializer;
pub mod services;
pub mod templates;
pub mod translations;

use std::path::{Path, PathBuf};

use crate::framework::yaml::{Node, parse};
use crate::index::Index;

/// The configuration files of a project that may hold services, parameters or routes: the files
/// directly in `config/` and everything below it, YAML and PHP.
pub(crate) fn config_files(index: &Index, extensions: &[&str]) -> Vec<PathBuf> {
    let dir = index.framework_root().join("config");
    index
        .files_below(&dir)
        .into_iter()
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| extensions.contains(&ext))
        })
        .collect()
}

/// The YAML of a file, when it has some.
pub(crate) fn yaml_of(index: &Index, path: &Path) -> Option<(std::sync::Arc<str>, Node)> {
    let text = index.read_text(path)?;
    let node = parse(&text)?;
    Some((text, node))
}

/// Whether a path of the project is one of the files the Symfony sections read.
pub(crate) fn is_config(root: &Path, path: &Path) -> bool {
    crate::framework::is_below(root, path, "config")
        || crate::framework::is_below(root, path, "src")
        || crate::framework::is_below(root, path, "templates")
        || crate::framework::is_below(root, path, "translations")
}
