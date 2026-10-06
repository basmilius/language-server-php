//! A project: a folder with its Composer metadata, the language level that follows from it and the
//! index of its files, its packages and the standard library.

use std::path::{Path, PathBuf};

use php_syntax::PhpVersion;

use crate::composer::Composer;
use crate::framework::Frameworks;
use crate::index::{Index, Origin};
use crate::indexer::IndexedFile;
use crate::words::WordIndex;

pub struct Project {
    pub root: PathBuf,
    /// The workspace folder that brought this project in. It is `root` itself unless the project is
    /// a composer root found below a folder that has no `composer.json`.
    pub folder: PathBuf,
    pub composer: Option<Composer>,
    pub level: PhpVersion,
    /// The level came from `composer.json` and not from the settings.
    pub level_from_composer: bool,
    pub index: Index,
    /// The words of the project's own files, for searches. Built when the first one asks.
    pub words: WordIndex,
    /// The words of the files of the installed packages, for a search that asks for them.
    pub package_words: WordIndex,
}

impl Project {
    pub fn open(root: &Path, default_level: PhpVersion) -> Project {
        Project::open_in(root, root, default_level)
    }

    /// A composer root below a workspace folder.
    pub fn open_in(root: &Path, folder: &Path, default_level: PhpVersion) -> Project {
        let composer = Composer::load(root);
        let (level, level_from_composer) = level_for(composer.as_ref(), default_level);
        let mut project = Project {
            root: root.to_path_buf(),
            folder: folder.to_path_buf(),
            composer,
            level,
            level_from_composer,
            index: Index::new(level),
            words: WordIndex::default(),
            package_words: WordIndex::default(),
        };
        project.configure_index();
        project
    }

    /// Tells the index which frameworks the project has. A new index needs it again.
    pub fn configure_index(&mut self) {
        let frameworks = self.composer.as_ref().map(Frameworks::detect).unwrap_or_default();
        self.index.set_frameworks(&self.root, frameworks);
    }

    /// An empty index for the same project, after `composer.json` or the packages changed.
    pub fn reset_index(&mut self) {
        self.index = Index::new(self.level);
        self.configure_index();
    }

    /// A project of nothing: the files that belong to no folder the client opened.
    pub fn loose(default_level: PhpVersion) -> Project {
        Project {
            root: PathBuf::new(),
            folder: PathBuf::new(),
            composer: None,
            level: default_level,
            level_from_composer: false,
            index: Index::new(default_level),
            words: WordIndex::default(),
            package_words: WordIndex::default(),
        }
    }

    /// Reads `composer.json` again. Returns whether anything the index depends on changed.
    pub fn reload_composer(&mut self, default_level: PhpVersion) -> bool {
        let composer = Composer::load(&self.root);
        let (level, from_composer) = level_for(composer.as_ref(), default_level);
        let changed = composer != self.composer || level != self.level;
        self.composer = composer;
        self.level = level;
        self.level_from_composer = from_composer;
        self.index.level = level;
        self.configure_index();
        changed
    }

    /// Whether this project was found below its workspace folder instead of being one.
    pub fn is_nested(&self) -> bool {
        self.root != self.folder
    }

    pub fn contains(&self, path: &Path) -> bool {
        !self.root.as_os_str().is_empty() && path.starts_with(&self.root)
    }

    pub fn origin_of(&self, path: &Path) -> Origin {
        match &self.composer {
            Some(composer) if path.starts_with(&composer.vendor_dir) => Origin::Vendor,
            _ => Origin::Project,
        }
    }

    /// Where the extracted declarations of this project are cached inside a storage folder.
    pub fn cache_path(&self, storage: &Path) -> PathBuf {
        let name = crate::cache::path_key(&self.root);
        storage.join("cache").join(format!("project-{name}.bin"))
    }

    /// Where the words of the project's own files are kept, or those of its packages.
    pub fn words_path(&self, storage: &Path, packages: bool) -> PathBuf {
        let name = crate::cache::path_key(&self.root);
        let kind = if packages { "package-words" } else { "words" };
        storage.join("cache").join(format!("{kind}-{name}.bin"))
    }

    /// The stub folders this project shows.
    pub fn extensions(&self) -> Vec<String> {
        let required = self
            .composer
            .as_ref()
            .map(|composer| composer.extensions.clone())
            .unwrap_or_default();
        crate::stubs::selected_extensions(&required)
    }

    pub fn apply(&mut self, files: Vec<IndexedFile>) {
        for file in files {
            self.index
                .set_indexed(file.path, file.origin, file.source, file.summary);
        }
    }
}

fn level_for(composer: Option<&Composer>, default_level: PhpVersion) -> (PhpVersion, bool) {
    match composer.and_then(|composer| composer.php_level) {
        Some(level) => (level, true),
        None => (default_level, false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn the_level_follows_composer_and_falls_back_to_the_default() {
        let dir = tempfile::tempdir().expect("a temp dir");
        fs::write(dir.path().join("composer.json"), r#"{"require":{"php":"^8.1"}}"#).expect("written");
        let project = Project::open(dir.path(), PhpVersion::V8_4);
        assert_eq!(project.level, PhpVersion::V8_1);
        assert!(project.level_from_composer);
        assert_eq!(project.index.level, PhpVersion::V8_1);

        fs::write(
            dir.path().join("composer.json"),
            r#"{"require":{"php":"^8.1"},"config":{"platform":{"php":"8.3"}}}"#,
        )
        .expect("written");
        let mut project = project;
        assert!(project.reload_composer(PhpVersion::V8_4));
        assert_eq!(project.level, PhpVersion::V8_3);

        let empty = tempfile::tempdir().expect("a temp dir");
        let bare = Project::open(empty.path(), PhpVersion::V8_4);
        assert_eq!(bare.level, PhpVersion::V8_4);
        assert!(!bare.level_from_composer);
    }

    #[test]
    fn tells_vendor_files_from_project_files() {
        let dir = tempfile::tempdir().expect("a temp dir");
        fs::write(dir.path().join("composer.json"), "{}").expect("written");
        let project = Project::open(dir.path(), PhpVersion::V8_4);
        assert_eq!(project.origin_of(&dir.path().join("src/A.php")), Origin::Project);
        assert_eq!(project.origin_of(&dir.path().join("vendor/a/b.php")), Origin::Vendor);
    }
}
