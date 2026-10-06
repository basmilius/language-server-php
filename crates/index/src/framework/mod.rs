//! What the frameworks a project installed add to PHP that its declarations do not say: the members
//! Eloquent and the facades make up at run time, and the names of config keys, routes, views,
//! translations and services that strings refer to.
//!
//! Nothing here activates in a project that does not install the framework: [`Frameworks`] comes from
//! `composer.json` and the installed packages, and every entry point asks it first. What the
//! frameworks declare themselves (docblocks, generics, attributes) is read from the index like any
//! other code. What they leave to run time lives in the overlays next to this file.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::composer::Composer;

pub mod abilities;
pub mod auth;
pub mod config;
pub mod container;
pub mod eloquent;
pub mod env;
pub mod facade;
pub mod inflect;
pub mod keys;
pub mod keytree;
pub mod layouts;
pub mod migrations;
pub mod overlay;
pub mod routes;
pub mod source;
pub mod symfony;
pub mod translations;
pub mod twig;
pub mod validation;
pub mod views;
pub mod yaml;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

/// The frameworks a project has, from what it requires and has installed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frameworks {
    /// `laravel/framework`: the conventions of an application (config, routes, views, translations).
    pub laravel: bool,
    /// `illuminate/support` or the framework: the facades.
    pub facades: bool,
    /// `illuminate/database` or the framework: Eloquent models.
    pub eloquent: bool,
    /// `symfony/framework-bundle` or `symfony/symfony`.
    pub symfony: bool,
    /// `doctrine/orm`.
    pub doctrine: bool,
    /// `twig/twig`.
    pub twig: bool,
}

impl Frameworks {
    pub fn detect(composer: &Composer) -> Frameworks {
        let laravel = composer.has_package("laravel/framework");
        Frameworks {
            laravel,
            facades: laravel || composer.has_package("illuminate/support"),
            eloquent: laravel || composer.has_package("illuminate/database"),
            symfony: composer.has_package("symfony/framework-bundle") || composer.has_package("symfony/symfony"),
            doctrine: composer.has_package("doctrine/orm"),
            twig: composer.has_package("twig/twig"),
        }
    }

    /// Every framework on, for tests and tools that read a project without Composer metadata.
    pub fn all() -> Frameworks {
        Frameworks {
            laravel: true,
            facades: true,
            eloquent: true,
            symfony: true,
            doctrine: true,
            twig: true,
        }
    }

    pub fn any(&self) -> bool {
        *self != Frameworks::default()
    }
}

/// Something the framework layer works out from the files of a project once and keeps until a file
/// it read changes.
pub trait Section: Any + Send + Sync + Sized {
    fn build(index: &crate::index::Index) -> Self;

    /// Whether a change to this file can change what the section holds.
    fn depends_on(root: &Path, path: &Path) -> bool;
}

struct Slot {
    id: TypeId,
    value: Arc<dyn Any + Send + Sync>,
    depends_on: fn(&Path, &Path) -> bool,
}

/// What an index keeps for the framework layer.
#[derive(Default)]
pub struct FrameworkState {
    pub frameworks: Frameworks,
    pub root: PathBuf,
    /// The text of the documents a person has open, which win over the disk.
    open: HashMap<PathBuf, Arc<str>>,
    sections: Mutex<Vec<Slot>>,
}

impl FrameworkState {
    pub fn configure(&mut self, root: &Path, frameworks: Frameworks) {
        self.root = root.to_path_buf();
        self.frameworks = frameworks;
        self.forget_all();
    }

    pub fn set_open_text(&mut self, path: &Path, text: Option<Arc<str>>) {
        match text {
            Some(text) => {
                self.open.insert(path.to_path_buf(), text);
            }
            None => {
                self.open.remove(path);
            }
        }
        self.forget(path);
    }

    /// The text of a file as the person sees it: the open document, else the disk.
    pub fn read_text(&self, path: &Path) -> Option<Arc<str>> {
        if let Some(text) = self.open.get(path) {
            return Some(text.clone());
        }
        let bytes = std::fs::read(path).ok()?;
        Some(Arc::from(String::from_utf8_lossy(&bytes).as_ref()))
    }

    /// The files below a folder, on disk and among the open documents.
    pub fn files_below(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = walkdir::WalkDir::new(dir)
            .into_iter()
            .flatten()
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .collect();
        files.extend(self.open.keys().filter(|path| path.starts_with(dir)).cloned());
        files.sort();
        files.dedup();
        files
    }

    /// The files directly in a folder, on disk and among the open documents.
    pub fn files_in(&self, dir: &Path) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .collect();
        files.extend(self.open.keys().filter(|path| path.parent() == Some(dir)).cloned());
        files.sort();
        files.dedup();
        files
    }

    /// Whether a file exists on disk or is an open document.
    pub fn file_exists(&self, path: &Path) -> bool {
        self.open.contains_key(path) || path.is_file()
    }

    /// Drops what was worked out from a file that changed.
    pub fn forget(&mut self, path: &Path) {
        if !self.frameworks.any() {
            return;
        }
        let root = self.root.clone();
        if let Ok(slots) = self.sections.get_mut() {
            slots.retain(|slot| !(slot.depends_on)(&root, path));
        }
    }

    fn forget_all(&mut self) {
        if let Ok(slots) = self.sections.get_mut() {
            slots.clear();
        }
    }

    pub(crate) fn find<S: Section>(&self) -> Option<Arc<S>> {
        let slots = self.sections.lock().ok()?;
        let slot = slots.iter().find(|slot| slot.id == TypeId::of::<S>())?;
        slot.value.clone().downcast::<S>().ok()
    }

    pub(crate) fn store<S: Section>(&self, value: Arc<S>) {
        if let Ok(mut slots) = self.sections.lock() {
            if slots.iter().all(|slot| slot.id != TypeId::of::<S>()) {
                slots.push(Slot {
                    id: TypeId::of::<S>(),
                    value,
                    depends_on: S::depends_on,
                });
            }
        }
    }
}

impl crate::index::Index {
    pub fn frameworks(&self) -> Frameworks {
        self.framework.frameworks
    }

    /// The folder of the project the framework conventions are read from.
    pub fn framework_root(&self) -> &Path {
        &self.framework.root
    }

    pub fn set_frameworks(&mut self, root: &Path, frameworks: Frameworks) {
        self.framework.configure(root, frameworks);
    }

    pub fn set_open_text(&mut self, path: &Path, text: Option<Arc<str>>) {
        self.framework.set_open_text(path, text);
    }

    /// A file of the project changed on disk, whatever its kind: a `.env`, a translation file or a
    /// template. PHP files tell the index themselves.
    pub fn framework_file_changed(&mut self, path: &Path) {
        self.framework.forget(path);
    }

    pub fn read_text(&self, path: &Path) -> Option<Arc<str>> {
        self.framework.read_text(path)
    }

    /// The files below a folder of the project, on disk and open.
    pub fn files_below(&self, dir: &Path) -> Vec<PathBuf> {
        self.framework.files_below(dir)
    }

    pub fn file_exists(&self, path: &Path) -> bool {
        self.framework.file_exists(path)
    }

    /// The files directly in a folder of the project, on disk and open.
    pub fn files_in(&self, dir: &Path) -> Vec<PathBuf> {
        self.framework.files_in(dir)
    }

    /// A section of the framework layer, built the first time it is asked for.
    pub fn section<S: Section>(&self) -> Arc<S> {
        if let Some(found) = self.framework.find::<S>() {
            return found;
        }
        let built = Arc::new(S::build(self));
        self.framework.store(built.clone());
        self.framework.find::<S>().unwrap_or(built)
    }
}

/// Whether a path sits below a folder of the project, with the folder given relative to it.
pub fn is_below(root: &Path, path: &Path, folder: &str) -> bool {
    path.strip_prefix(root)
        .is_ok_and(|relative| relative.starts_with(folder))
}

/// A PHP file of the project that is not a package's.
pub fn is_project_php(root: &Path, path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("php"))
        && path.starts_with(root)
        && !is_below(root, path, "vendor")
}

/// Adds the members the frameworks make up at run time to a type's methods.
pub(crate) fn extend_methods<'a>(
    index: &'a crate::index::Index,
    ancestors: &[crate::hierarchy::Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<crate::hierarchy::Found<'a, crate::model::Method>>,
    names: &mut std::collections::HashSet<String>,
) {
    if index.framework.frameworks.facades {
        facade::extend(index, ancestors, only, out, names);
    }
    if index.framework.frameworks.eloquent {
        eloquent::extend_methods(index, ancestors, only, out, names);
    }
    if index.framework.frameworks.laravel || index.framework.frameworks.facades {
        forward_calls(index, ancestors, only, out, names);
    }
    if index.framework.frameworks.doctrine {
        symfony::doctrine::extend_methods(index, ancestors, only, out, names);
    }
}

/// Adds the properties the frameworks make up at run time.
pub(crate) fn extend_properties<'a>(
    index: &'a crate::index::Index,
    ancestors: &[crate::hierarchy::Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<crate::hierarchy::Found<'a, crate::model::Property>>,
    names: &mut std::collections::HashSet<String>,
) {
    if index.framework.frameworks.eloquent {
        eloquent::extend_properties(index, ancestors, only, out, names);
    }
    if index.framework.frameworks.doctrine {
        symfony::doctrine::extend_properties(index, ancestors, only, out);
    }
}

/// What a class passes on through `__call` to another class, which the overlay names.
fn forward_calls<'a>(
    index: &'a crate::index::Index,
    ancestors: &[crate::hierarchy::Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<crate::hierarchy::Found<'a, crate::model::Method>>,
    names: &mut std::collections::HashSet<String>,
) {
    for ancestor in ancestors {
        let Some(target) = overlay::forwards_of(index, &ancestor.class.decl.name) else {
            continue;
        };
        let target_type = crate::types::Type::class(target);
        let forwarded = match only {
            Some(name) => index.find_method(&target_type, name).into_iter().collect(),
            None => index.methods(&target_type),
        };
        for found in forwarded {
            if found.member.visibility != crate::model::Visibility::Public
                || found.member.name.starts_with("__")
                || !names.insert(found.member.name.to_ascii_lowercase())
            {
                continue;
            }
            out.push(crate::hierarchy::Found {
                mixin: true,
                static_as: found.static_as.clone().or_else(|| Some(target_type.clone())),
                ..found
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn detect(require: serde_json::Value) -> Frameworks {
        Frameworks::detect(&Composer::from_json(Path::new("/p"), &json!({ "require": require })))
    }

    #[test]
    fn a_framework_is_detected_from_what_the_project_requires() {
        let laravel = detect(json!({ "laravel/framework": "^13.0" }));
        assert!(laravel.laravel && laravel.facades && laravel.eloquent && !laravel.symfony);
        let eloquent = detect(json!({ "illuminate/database": "^11.0" }));
        assert!(eloquent.eloquent && !eloquent.laravel && !eloquent.facades);
        let support = detect(json!({ "illuminate/support": "^11.0" }));
        assert!(support.facades && !support.eloquent);
        let symfony = detect(json!({ "symfony/framework-bundle": "8.1.*", "doctrine/orm": "^3", "twig/twig": "^3" }));
        assert!(symfony.symfony && symfony.doctrine && symfony.twig && !symfony.laravel);
        assert!(!detect(json!({ "php": "^8.3", "monolog/monolog": "^3" })).any());
    }

    #[test]
    fn nothing_is_made_up_in_a_project_without_the_framework() {
        let mut index = crate::framework::testing::project(&[
            (
                "vendor/laravel/Facade.php",
                "<?php namespace Illuminate\\Support\\Facades; abstract class Facade { public static function __callStatic($m, $a) {} }",
            ),
            (
                "app/Facades/F.php",
                "<?php namespace App\\Facades; /** @see \\App\\Real */ class F extends \\Illuminate\\Support\\Facades\\Facade {}",
            ),
            (
                "app/Real.php",
                "<?php namespace App; class Real { public function run() {} }",
            ),
        ]);
        let ty = crate::types::Type::class("App\\Facades\\F");
        assert!(index.find_method(&ty, "run").is_some());
        index.set_frameworks(Path::new(testing::ROOT), Frameworks::default());
        assert!(index.find_method(&ty, "run").is_none());
        assert!(overlay::markers_for(&index, None, None, "config").is_empty());
        assert!(overlay::markers_for(&index, Some("Illuminate\\Support\\Facades\\Config"), None, "get").is_empty());
    }
}
