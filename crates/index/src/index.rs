//! The index of one project: the declarations of its files, of the packages it installed and of the
//! standard library, with the maps that find a declaration by name.
//!
//! A file whose declarations are in the cache file stays there until something asks for them: the
//! index keeps the names of its classes, functions and constants, which is all that finding a
//! declaration by name or a class by what it extends needs. [`Index::trim`] puts the files that
//! have not been used lately back.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use php_syntax::PhpVersion;
use serde::{Deserialize, Serialize};

use crate::model::{ClassDecl, ClassSummary, ConstDecl, FileSummary, FileSymbols, Function, NameSummary};
use crate::store::SymbolSource;
use crate::types::Name;

pub type FileId = u32;

/// Where a file comes from. A project file wins over a package that declares the same name, and a
/// package over the standard library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Origin {
    Project,
    Vendor,
    Stub,
}

/// Something that happened to a file, in the order things happen, so the oldest use goes first.
static CLOCK: AtomicU64 = AtomicU64::new(1);

pub struct FileEntry {
    pub path: PathBuf,
    pub origin: Origin,
    summary: Arc<FileSummary>,
    source: SymbolSource,
    loaded: OnceLock<Arc<FileSymbols>>,
    last_used: AtomicU64,
    /// Read from disk by the indexer, so the cache file can stand in for what is in memory.
    from_disk: bool,
}

impl std::fmt::Debug for FileEntry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FileEntry")
            .field("path", &self.path)
            .field("origin", &self.origin)
            .field("loaded", &self.loaded.get().is_some())
            .finish()
    }
}

impl FileEntry {
    fn new(
        path: PathBuf,
        origin: Origin,
        source: SymbolSource,
        summary: Arc<FileSummary>,
        from_disk: bool,
    ) -> FileEntry {
        let loaded = OnceLock::new();
        if let SymbolSource::Memory(symbols) = &source {
            let _ = loaded.set(symbols.clone());
        }
        FileEntry {
            path,
            origin,
            summary,
            source,
            loaded,
            last_used: AtomicU64::new(CLOCK.fetch_add(1, Ordering::Relaxed)),
            from_disk,
        }
    }

    /// The declarations of the file, read from the cache file the first time they are asked for. A
    /// file whose declarations cannot be read is read from its source again, and failing that is
    /// empty.
    pub fn symbols(&self) -> &FileSymbols {
        self.last_used
            .store(CLOCK.fetch_add(1, Ordering::Relaxed), Ordering::Relaxed);
        self.loaded.get_or_init(|| {
            self.source
                .load()
                .or_else(|| crate::indexer::index_file(&self.path, self.origin).map(Arc::new))
                .unwrap_or_default()
        })
    }

    /// The names the file declares, which are known without reading its declarations.
    pub fn summary(&self) -> &FileSummary {
        &self.summary
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded.get().is_some()
    }
}

/// A file of the standard library, ready to be added to the index of a project.
#[derive(Clone, Debug)]
pub struct StubFile {
    pub path: PathBuf,
    /// The normalized name of the extension folder.
    pub extension: String,
    pub source: SymbolSource,
    pub summary: Arc<FileSummary>,
}

impl StubFile {
    pub fn new(path: PathBuf, extension: String, symbols: Arc<FileSymbols>) -> StubFile {
        let summary = Arc::new(FileSummary::of(&symbols));
        StubFile {
            path,
            extension,
            source: SymbolSource::Memory(symbols),
            summary,
        }
    }

    pub fn from_indexed(file: crate::indexer::IndexedFile) -> StubFile {
        StubFile {
            path: file.path,
            extension: file.extension.unwrap_or_default(),
            source: file.source,
            summary: file.summary,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Class<'a> {
    pub file: &'a FileEntry,
    pub decl: &'a ClassDecl,
}

#[derive(Clone, Copy)]
pub struct FunctionRef<'a> {
    pub file: &'a FileEntry,
    pub decl: &'a Function,
}

#[derive(Clone, Copy)]
pub struct ConstRef<'a> {
    pub file: &'a FileEntry,
    pub decl: &'a ConstDecl,
}

/// A class by name, from the names of its file. Reading the rest of it is [`ClassName::load`].
#[derive(Clone, Copy)]
pub struct ClassName<'a> {
    pub file: &'a FileEntry,
    pub summary: &'a ClassSummary,
    slot: u32,
}

impl<'a> ClassName<'a> {
    pub fn load(&self) -> Option<Class<'a>> {
        let decl = self.file.symbols().classes.get(self.slot as usize)?;
        Some(Class { file: self.file, decl })
    }
}

/// A function or a constant by name.
#[derive(Clone, Copy)]
pub struct DeclName<'a> {
    pub file: &'a FileEntry,
    pub summary: &'a NameSummary,
    slot: u32,
}

impl<'a> DeclName<'a> {
    pub fn load_function(&self) -> Option<FunctionRef<'a>> {
        let decl = self.file.symbols().functions.get(self.slot as usize)?;
        Some(FunctionRef { file: self.file, decl })
    }

    pub fn load_constant(&self) -> Option<ConstRef<'a>> {
        let decl = self.file.symbols().constants.get(self.slot as usize)?;
        Some(ConstRef { file: self.file, decl })
    }
}

type Slot = (FileId, u32);

pub struct Index {
    pub level: PhpVersion,
    files: Vec<Option<FileEntry>>,
    by_path: HashMap<PathBuf, FileId>,
    classes: HashMap<String, Vec<Slot>>,
    functions: HashMap<String, Vec<Slot>>,
    constants: HashMap<String, Vec<Slot>>,
    /// Direct subtypes by the lowercase name of the class they extend, implement or use.
    subtypes: HashMap<String, Vec<Slot>>,
    /// The folders of the standard library this index shows, by normalized name.
    pub stub_extensions: Vec<String>,
    pub(crate) framework: crate::framework::FrameworkState,
}

fn class_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// A constant's namespace is case-insensitive and its own name is not.
fn constant_key(name: &str) -> String {
    match name.rsplit_once('\\') {
        Some((namespace, short)) => format!("{}\\{short}", namespace.to_ascii_lowercase()),
        None => name.to_string(),
    }
}

impl Index {
    pub fn new(level: PhpVersion) -> Index {
        Index {
            level,
            files: Vec::new(),
            by_path: HashMap::new(),
            classes: HashMap::new(),
            functions: HashMap::new(),
            constants: HashMap::new(),
            subtypes: HashMap::new(),
            stub_extensions: Vec::new(),
            framework: crate::framework::FrameworkState::default(),
        }
    }

    pub fn file_count(&self) -> usize {
        self.by_path.len()
    }

    pub fn class_count(&self) -> usize {
        self.classes.values().map(Vec::len).sum()
    }

    pub fn function_count(&self) -> usize {
        self.functions.values().map(Vec::len).sum()
    }

    pub fn file(&self, path: &Path) -> Option<&FileEntry> {
        let id = *self.by_path.get(path)?;
        self.files.get(id as usize)?.as_ref()
    }

    pub fn contains_file(&self, path: &Path) -> bool {
        self.by_path.contains_key(path)
    }

    /// Adds a file, or replaces what the index knew of it.
    pub fn set_file(&mut self, path: PathBuf, origin: Origin, symbols: Arc<FileSymbols>) {
        let summary = Arc::new(FileSummary::of(&symbols));
        self.insert(FileEntry::new(
            path,
            origin,
            SymbolSource::Memory(symbols),
            summary,
            false,
        ));
    }

    /// Adds a file the indexer read, whose declarations may stay in the cache file.
    pub fn set_indexed(&mut self, path: PathBuf, origin: Origin, source: SymbolSource, summary: Arc<FileSummary>) {
        self.insert(FileEntry::new(path, origin, source, summary, true));
    }

    fn insert(&mut self, entry: FileEntry) {
        self.remove_file(&entry.path);
        if entry.origin == Origin::Project {
            self.framework.forget(&entry.path);
        }
        let id = self.files.len() as FileId;
        for (index, class) in entry.summary.classes.iter().enumerate() {
            let slot = (id, index as u32);
            self.classes.entry(class_key(&class.name)).or_default().push(slot);
            for parent in &class.parents {
                self.subtypes.entry(class_key(parent)).or_default().push(slot);
            }
        }
        for (index, function) in entry.summary.functions.iter().enumerate() {
            self.functions
                .entry(class_key(&function.name))
                .or_default()
                .push((id, index as u32));
        }
        for (index, constant) in entry.summary.constants.iter().enumerate() {
            self.constants
                .entry(constant_key(&constant.name))
                .or_default()
                .push((id, index as u32));
        }
        self.by_path.insert(entry.path.clone(), id);
        self.files.push(Some(entry));
    }

    pub fn remove_file(&mut self, path: &Path) {
        let Some(id) = self.by_path.remove(path) else {
            return;
        };
        let Some(entry) = self.files[id as usize].take() else {
            return;
        };
        if entry.origin == Origin::Project {
            self.framework.forget(path);
        }
        let drop_slots = |map: &mut HashMap<String, Vec<Slot>>, key: String| {
            if let Some(slots) = map.get_mut(&key) {
                slots.retain(|(file, _)| *file != id);
                if slots.is_empty() {
                    map.remove(&key);
                }
            }
        };
        for class in &entry.summary.classes {
            drop_slots(&mut self.classes, class_key(&class.name));
            for parent in &class.parents {
                drop_slots(&mut self.subtypes, class_key(parent));
            }
        }
        for function in &entry.summary.functions {
            drop_slots(&mut self.functions, class_key(&function.name));
        }
        for constant in &entry.summary.constants {
            drop_slots(&mut self.constants, constant_key(&constant.name));
        }
    }

    /// Lets a file the indexer read be read from the cache file from now on, which frees what is in
    /// memory for it. Files that were replaced since, by an open document or an edit, are left alone.
    pub fn move_to_cache(&mut self, path: &Path, source: SymbolSource) {
        let Some(id) = self.by_path.get(path) else {
            return;
        };
        let Some(entry) = self.files[*id as usize].as_mut() else {
            return;
        };
        if !entry.from_disk {
            return;
        }
        entry.source = source;
        entry.loaded = OnceLock::new();
    }

    /// Frees the declarations of the files read from the cache file that were used longest ago, down
    /// to `keep` of them. They are read again when something asks.
    pub fn trim(&mut self, keep: usize) {
        let mut loaded: Vec<(u64, usize)> = self
            .files
            .iter()
            .enumerate()
            .filter_map(|(id, entry)| {
                let entry = entry.as_ref()?;
                (entry.source.is_lazy() && entry.is_loaded()).then(|| (entry.last_used.load(Ordering::Relaxed), id))
            })
            .collect();
        if loaded.len() <= keep {
            return;
        }
        loaded.sort_unstable();
        let excess = loaded.len() - keep;
        for (_, id) in loaded.into_iter().take(excess) {
            if let Some(entry) = self.files[id].as_mut() {
                entry.loaded = OnceLock::new();
            }
        }
    }

    /// How many files have their declarations in memory.
    pub fn loaded_count(&self) -> usize {
        self.files.iter().flatten().filter(|entry| entry.is_loaded()).count()
    }

    /// Adds the standard library files of the extensions this index shows, and forgets the ones it
    /// showed before.
    pub fn set_stubs(&mut self, stubs: &[StubFile], extensions: &[String]) {
        let old: Vec<PathBuf> = self
            .files
            .iter()
            .flatten()
            .filter(|entry| entry.origin == Origin::Stub)
            .map(|entry| entry.path.clone())
            .collect();
        for path in old {
            self.remove_file(&path);
        }
        self.stub_extensions = extensions.to_vec();
        for stub in stubs {
            if extensions.contains(&stub.extension) || stub.extension.is_empty() {
                self.insert(FileEntry::new(
                    stub.path.clone(),
                    Origin::Stub,
                    stub.source.clone(),
                    stub.summary.clone(),
                    true,
                ));
            }
        }
    }

    fn pick<'a, T>(
        &'a self,
        slots: &[Slot],
        get: impl Fn(&'a FileEntry, u32) -> Option<(&'a T, bool)>,
    ) -> Option<(&'a FileEntry, &'a T)> {
        let mut best: Option<(&'a FileEntry, &'a T)> = None;
        for (file, index) in slots {
            let Some(entry) = self.files.get(*file as usize).and_then(Option::as_ref) else {
                continue;
            };
            // A file that cannot win is not read.
            if best.is_some_and(|(current, _)| current.origin <= entry.origin) {
                continue;
            }
            let Some((decl, available)) = get(entry, *index) else {
                continue;
            };
            if !available {
                continue;
            }
            best = Some((entry, decl));
        }
        best
    }

    pub fn class(&self, name: &str) -> Option<Class<'_>> {
        let name = name.trim_start_matches('\\');
        let Some(slots) = self.classes.get(&class_key(name)) else {
            return self.aliased_class(name);
        };
        self.pick(slots, |entry, index| {
            let decl = entry.symbols().classes.get(index as usize)?;
            // A file that changed since it was indexed may have its declarations elsewhere.
            if !decl.name.eq_ignore_ascii_case(name) {
                return None;
            }
            Some((decl, decl.availability.contains(self.level)))
        })
        .map(|(file, decl)| Class { file, decl })
    }

    /// The class a global alias of Laravel's stands for (`DB`), for a name that has no namespace.
    fn aliased_class(&self, name: &str) -> Option<Class<'_>> {
        if name.contains('\\') || !self.frameworks().laravel {
            return None;
        }
        let aliases = self.section::<crate::framework::aliases::ClassAliases>();
        let target = aliases.target(name)?;
        // A class an alias names is never itself an alias without a namespace.
        if !target.contains('\\') {
            return None;
        }
        self.class(target)
    }

    /// How many files declare a class of this name that exists at the level. More than one means
    /// the declarations are conditional, and which of them runs is not known.
    pub fn class_declarations(&self, name: &str) -> usize {
        let name = name.trim_start_matches('\\');
        let Some(slots) = self.classes.get(&class_key(name)) else {
            return 0;
        };
        slots
            .iter()
            .filter(|(file, index)| {
                self.files
                    .get(*file as usize)
                    .and_then(Option::as_ref)
                    .and_then(|entry| entry.summary.classes.get(*index as usize))
                    .is_some_and(|class| class.availability.contains(self.level))
            })
            .count()
    }

    /// Whether any file defines constants at run time, so that a constant the index does not hold
    /// may still be there.
    pub fn has_dynamic_defines(&self) -> bool {
        self.files.iter().flatten().any(|entry| entry.summary.dynamic_define)
    }

    pub fn function(&self, name: &str) -> Option<FunctionRef<'_>> {
        let name = name.trim_start_matches('\\');
        let slots = self.functions.get(&class_key(name))?;
        self.pick(slots, |entry, index| {
            let decl = entry.symbols().functions.get(index as usize)?;
            if !decl.name.eq_ignore_ascii_case(name) {
                return None;
            }
            Some((decl, decl.availability.contains(self.level)))
        })
        .map(|(file, decl)| FunctionRef { file, decl })
    }

    /// Every declaration of a function that exists at the level, from the origin that wins. The
    /// standard library declares some functions once per way of calling them.
    pub fn function_overloads(&self, name: &str) -> Vec<FunctionRef<'_>> {
        let Some(slots) = self.functions.get(&class_key(name.trim_start_matches('\\'))) else {
            return Vec::new();
        };
        let mut found: Vec<FunctionRef<'_>> = slots
            .iter()
            .filter_map(|(file, index)| {
                let entry = self.files.get(*file as usize)?.as_ref()?;
                let decl = entry.symbols().functions.get(*index as usize)?;
                decl.availability
                    .contains(self.level)
                    .then_some(FunctionRef { file: entry, decl })
            })
            .collect();
        if let Some(best) = found.iter().map(|function| function.file.origin).min() {
            found.retain(|function| function.file.origin == best);
        }
        found
    }

    pub fn constant(&self, name: &str) -> Option<ConstRef<'_>> {
        let name = name.trim_start_matches('\\');
        let slots = self.constants.get(&constant_key(name))?;
        self.pick(slots, |entry, index| {
            let decl = entry.symbols().constants.get(index as usize)?;
            if constant_key(&decl.name) != constant_key(name) {
                return None;
            }
            Some((decl, decl.availability.contains(self.level)))
        })
        .map(|(file, decl)| ConstRef { file, decl })
    }

    /// The first function of a list of candidate names, which is how PHP falls back to the global
    /// namespace.
    pub fn first_function(&self, candidates: &[Name]) -> Option<FunctionRef<'_>> {
        candidates.iter().find_map(|name| self.function(name))
    }

    pub fn first_constant(&self, candidates: &[Name]) -> Option<ConstRef<'_>> {
        candidates.iter().find_map(|name| self.constant(name))
    }

    /// The classes that exist at the level, by name. Nothing is read until [`ClassName::load`].
    pub fn class_names(&self) -> impl Iterator<Item = ClassName<'_>> {
        self.files
            .iter()
            .flatten()
            .flat_map(|file| {
                file.summary
                    .classes
                    .iter()
                    .enumerate()
                    .map(move |(slot, summary)| ClassName {
                        file,
                        summary,
                        slot: slot as u32,
                    })
            })
            .filter(|class| class.summary.availability.contains(self.level))
    }

    pub fn function_names(&self) -> impl Iterator<Item = DeclName<'_>> {
        self.files
            .iter()
            .flatten()
            .flat_map(|file| {
                file.summary
                    .functions
                    .iter()
                    .enumerate()
                    .map(move |(slot, summary)| DeclName {
                        file,
                        summary,
                        slot: slot as u32,
                    })
            })
            .filter(|function| function.summary.availability.contains(self.level))
    }

    pub fn constant_names(&self) -> impl Iterator<Item = DeclName<'_>> {
        self.files
            .iter()
            .flatten()
            .flat_map(|file| {
                file.summary
                    .constants
                    .iter()
                    .enumerate()
                    .map(move |(slot, summary)| DeclName {
                        file,
                        summary,
                        slot: slot as u32,
                    })
            })
            .filter(|constant| constant.summary.availability.contains(self.level))
    }

    /// The classes that name another class in `extends`, `implements` or a trait `use`.
    pub fn direct_subtypes(&self, name: &str) -> Vec<Class<'_>> {
        let Some(slots) = self.subtypes.get(&class_key(name.trim_start_matches('\\'))) else {
            return Vec::new();
        };
        slots
            .iter()
            .filter_map(|(file, index)| {
                let entry = self.files.get(*file as usize)?.as_ref()?;
                if !entry
                    .summary
                    .classes
                    .get(*index as usize)?
                    .availability
                    .contains(self.level)
                {
                    return None;
                }
                let decl = entry.symbols().classes.get(*index as usize)?;
                Some(Class { file: entry, decl })
            })
            .collect()
    }

    /// Every class below another one, through any number of steps.
    pub fn all_subtypes(&self, name: &str) -> Vec<Class<'_>> {
        let mut out: Vec<Class<'_>> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut queue = vec![name.to_string()];
        while let Some(current) = queue.pop() {
            for class in self.direct_subtypes(&current) {
                if seen.insert(class_key(&class.decl.name)) {
                    queue.push(class.decl.name.clone());
                    out.push(class);
                }
            }
        }
        out
    }

    /// Every file's path and origin.
    pub fn files(&self) -> impl Iterator<Item = &FileEntry> {
        self.files.iter().flatten()
    }

    /// The project files that declare a group, a dataset or a binding of a test case, with it.
    pub fn test_files(&self) -> impl Iterator<Item = (&FileEntry, &crate::test_facts::TestFacts)> {
        self.files
            .iter()
            .flatten()
            .filter(|entry| entry.origin == Origin::Project)
            .filter_map(|entry| Some((entry, entry.summary().tests.as_deref()?)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ExtractOptions, extract};
    use php_syntax::parse;

    fn symbols(text: &str, stub: bool) -> Arc<FileSymbols> {
        Arc::new(extract(&parse(text).syntax(), ExtractOptions { stub }))
    }

    #[test]
    fn finds_declarations_by_name_and_prefers_the_project() {
        let mut index = Index::new(PhpVersion::V8_4);
        index.set_file(
            PathBuf::from("/stubs/a.php"),
            Origin::Stub,
            symbols("<?php class Foo {} function strlen() {} const E = 1;", true),
        );
        index.set_file(
            PathBuf::from("/p/foo.php"),
            Origin::Project,
            symbols("<?php namespace P; class Foo {} function strlen() {}", false),
        );
        assert_eq!(index.class("foo").unwrap().file.origin, Origin::Stub);
        assert_eq!(index.class("P\\FOO").unwrap().file.origin, Origin::Project);
        assert_eq!(index.function("strlen").unwrap().file.origin, Origin::Stub);
        assert!(index.constant("E").is_some());
        assert!(index.constant("e").is_none());
        index.remove_file(Path::new("/stubs/a.php"));
        assert!(index.class("Foo").is_none());
    }

    #[test]
    fn hides_what_the_level_does_not_have() {
        let text = "<?php /** @since 8.2 */ class New82 {} /** @removed 8.0 */ class Gone80 {} class Always {}";
        let mut old = Index::new(PhpVersion::V8_1);
        old.set_file(PathBuf::from("/s.php"), Origin::Stub, symbols(text, true));
        assert!(old.class("New82").is_none());
        assert!(old.class("Gone80").is_none());
        assert!(old.class("Always").is_some());
        let mut new = Index::new(PhpVersion::V8_4);
        new.set_file(PathBuf::from("/s.php"), Origin::Stub, symbols(text, true));
        assert!(new.class("New82").is_some());
        assert_eq!(new.class_names().count(), 2);
    }

    #[test]
    fn tracks_subtypes_and_replaces_a_file() {
        let mut index = Index::new(PhpVersion::V8_4);
        index.set_file(
            PathBuf::from("/a.php"),
            Origin::Project,
            symbols(
                "<?php interface I {} class A implements I {} class B extends A {}",
                false,
            ),
        );
        let names: Vec<&str> = index
            .all_subtypes("I")
            .iter()
            .map(|class| class.decl.name.as_str())
            .collect();
        assert_eq!(names.len(), 2);
        index.set_file(
            PathBuf::from("/a.php"),
            Origin::Project,
            symbols("<?php interface I {} class A {}", false),
        );
        assert!(index.all_subtypes("I").is_empty());
        assert_eq!(index.class_count(), 2);
    }

    #[test]
    fn files_in_the_cache_are_read_when_asked_and_let_go_by_trim() {
        use crate::cache::Cache;
        use crate::indexer::{self, IndexEvent};
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut files = Vec::new();
        for number in 0..6 {
            let path = dir.path().join(format!("c{number}.php"));
            std::fs::write(&path, format!("<?php class C{number} extends Base {{}}")).expect("written");
            files.push((path, Origin::Project));
        }
        let cache = dir.path().join("cache.bin");
        let collect = || {
            let found = std::sync::Mutex::new(Vec::new());
            indexer::run(files.clone(), Some(&cache), None, 2, &|event| {
                if let IndexEvent::Files(mut batch) = event {
                    found.lock().expect("lock").append(&mut batch);
                }
            });
            found.into_inner().expect("lock")
        };
        collect();
        assert!(Cache::load(&cache).store.is_some());
        let mut index = Index::new(PhpVersion::V8_4);
        for file in collect() {
            index.set_indexed(file.path, file.origin, file.source, file.summary);
        }
        assert_eq!(index.loaded_count(), 0, "indexing a warm cache reads no declarations");
        assert_eq!(
            index.all_subtypes("Base").len(),
            6,
            "subtypes come from the names alone"
        );
        assert_eq!(index.loaded_count(), 6);
        assert!(index.class("C3").is_some());
        index.trim(2);
        assert_eq!(index.loaded_count(), 2);
        assert!(index.class("C0").is_some(), "a class that was let go is read again");
        assert_eq!(index.loaded_count(), 3);
    }

    #[test]
    fn a_stale_slot_does_not_find_another_declaration() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("a.php");
        std::fs::write(&path, "<?php class A {} class B {}").expect("written");
        let mut index = Index::new(PhpVersion::V8_4);
        let summary = Arc::new(FileSummary::of(&extract(
            &parse("<?php class A {} class B {}").syntax(),
            ExtractOptions::default(),
        )));
        // The file changed after it was indexed: the second class is gone.
        std::fs::write(&path, "<?php class A {}").expect("written");
        index.set_indexed(
            path.clone(),
            Origin::Project,
            SymbolSource::Parse { path, stub: false },
            summary,
        );
        assert!(index.class("A").is_some());
        assert!(index.class("B").is_none());
    }

    #[test]
    fn only_shows_the_selected_extensions() {
        let stubs = vec![
            StubFile::new(
                PathBuf::from("/s/standard/a.php"),
                "standard".into(),
                symbols("<?php function a() {}", true),
            ),
            StubFile::new(
                PathBuf::from("/s/swoole/b.php"),
                "swoole".into(),
                symbols("<?php function b() {}", true),
            ),
        ];
        let mut index = Index::new(PhpVersion::V8_4);
        index.set_stubs(&stubs, &["standard".to_string()]);
        assert!(index.function("a").is_some());
        assert!(index.function("b").is_none());
        index.set_stubs(&stubs, &["swoole".to_string()]);
        assert!(index.function("a").is_none());
        assert!(index.function("b").is_some());
    }
}
