//! Indexes many files at once: finds them, reads the declarations of each in parallel, and reuses
//! what the cache holds for files that did not change.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use php_syntax::parse;
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::cache::{Cache, CachedFile, Entry, Payload, Stamp, content_hash};
use crate::composer::Composer;
use crate::extract::{ExtractOptions, extract};
use crate::index::Origin;
use crate::model::{FileSummary, FileSymbols};
use crate::store::SymbolSource;
use crate::stubs;

/// Files larger than this are generated or data, and parsing them tells the index nothing.
const MAX_FILE_SIZE: u64 = 4 << 20;
const BATCH: usize = 256;

#[derive(Clone, Debug)]
pub struct IndexedFile {
    pub path: PathBuf,
    pub origin: Origin,
    /// For a stub, the normalized name of its extension folder.
    pub extension: Option<String>,
    /// Where the declarations are: in memory when the file was just read, in the cache file when
    /// the cache had them.
    pub source: SymbolSource,
    pub summary: Arc<FileSummary>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub files: usize,
    pub parsed: usize,
    pub from_cache: usize,
    pub discovery: Duration,
    pub total: Duration,
}

pub enum IndexEvent {
    Discovered(usize),
    Files(Vec<IndexedFile>),
    /// The cache was written again: these files can be read from the new cache file, which lets
    /// the declarations kept in memory go and the old cache file be released.
    Persisted(Vec<(PathBuf, SymbolSource)>),
    Finished(Stats),
}

/// Folders that never hold source a person works on.
fn skipped_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "node_modules" | "vendor")
}

fn is_php(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("php"))
}

/// How deep below a workspace folder a `composer.json` is looked for.
const MAX_COMPOSER_DEPTH: usize = 4;

/// The folders below a workspace folder that hold a `composer.json` of their own, for a workspace
/// folder that has none. The search stops at the first one on every branch and skips folders that
/// hold packages, tooling or dependencies. A folder starting with `~` is a backup by convention and
/// is left out, since indexing a copy of a project and its `vendor` costs as much as the project.
pub fn discover_composer_roots(folder: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if folder.join("composer.json").is_file() {
        return roots;
    }
    let mut walker = WalkDir::new(folder)
        .follow_links(false)
        .max_depth(MAX_COMPOSER_DEPTH)
        .into_iter();
    while let Some(entry) = walker.next() {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 || !entry.file_type().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if skipped_dir(&name) || name.starts_with('~') {
            walker.skip_current_dir();
        } else if entry.path().join("composer.json").is_file() {
            roots.push(entry.into_path());
            walker.skip_current_dir();
        }
    }
    roots.sort();
    roots
}

/// The PHP files of a project: everything under its folder except the vendor folder, generated
/// caches and the `skip` folders (composer projects of their own), then the files of the installed
/// packages that Composer would load.
pub fn discover_project(root: &Path, composer: Option<&Composer>, skip: &[PathBuf]) -> Vec<(PathBuf, Origin)> {
    let vendor_dir = composer.map(|composer| composer.vendor_dir.clone());
    let mut found: BTreeSet<PathBuf> = BTreeSet::new();
    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.depth() == 0 || !entry.file_type().is_dir() {
                return true;
            }
            let name = entry.file_name().to_string_lossy();
            let path = entry.path();
            if vendor_dir.as_deref() == Some(path) || skipped_dir(&name) || skip.iter().any(|skipped| skipped == path) {
                return false;
            }
            !(path.ends_with("storage/framework") || path.ends_with("bootstrap/cache") || path.ends_with("var/cache"))
        });
    for entry in walker.flatten() {
        if entry.file_type().is_file() && is_php(entry.path()) {
            found.insert(entry.into_path());
        }
    }
    let mut files: Vec<(PathBuf, Origin)> = found.into_iter().map(|path| (path, Origin::Project)).collect();

    if let Some(composer) = composer {
        let mut vendor: BTreeSet<PathBuf> = BTreeSet::new();
        for root in composer.vendor_roots() {
            if root.is_file() {
                vendor.insert(root);
                continue;
            }
            for entry in WalkDir::new(&root).follow_links(false).into_iter().flatten() {
                if entry.file_type().is_file() && is_php(entry.path()) && !composer.excluded(entry.path()) {
                    vendor.insert(entry.into_path());
                }
            }
        }
        files.extend(vendor.into_iter().map(|path| (path, Origin::Vendor)));
        let known: HashSet<PathBuf> = files.iter().map(|(path, _)| path.clone()).collect();
        let mut loaded: Vec<(PathBuf, Origin)> = composer_loaded(composer)
            .into_iter()
            .filter(|path| !known.contains(path) && !composer.excluded(path))
            .map(|path| {
                let own = path.starts_with(root) && !path.starts_with(&composer.vendor_dir);
                (path, if own { Origin::Project } else { Origin::Vendor })
            })
            .collect();
        loaded.sort();
        loaded.dedup();
        files.extend(loaded);
    }
    files
}

/// The files Composer loads that a walk of the folders leaves out: the project's own `files` and
/// `classmap` entries in a folder the walk skips, and the files the generated class map names
/// whatever their extension (`classmap` scanning takes `.inc` files too).
fn composer_loaded(composer: &Composer) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = composer
        .autoload
        .files
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    for entry in &composer.autoload.classmap {
        if entry.is_file() {
            out.push(entry.clone());
            continue;
        }
        for found in WalkDir::new(entry).follow_links(false).into_iter().flatten() {
            if found.file_type().is_file() && is_php(found.path()) {
                out.push(found.into_path());
            }
        }
    }
    out.extend(
        crate::composer::read_generated_classmap(&composer.vendor_dir)
            .into_values()
            .filter(|path| !is_php(path) && path.is_file()),
    );
    out
}

/// The PHP files of the extension folders of the stubs.
pub fn discover_stubs(root: &Path) -> Vec<(PathBuf, Origin)> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return files;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_dir() || !stubs::is_extension_folder(&name) {
            continue;
        }
        for file in WalkDir::new(entry.path()).into_iter().flatten() {
            if file.file_type().is_file() && is_php(file.path()) {
                files.push((file.into_path(), Origin::Stub));
            }
        }
    }
    files.sort();
    files
}

/// The declarations of a text.
pub fn extract_text(text: &str, stub: bool) -> FileSymbols {
    extract(&parse(text).syntax(), ExtractOptions { stub })
}

/// Reads and extracts one file from disk.
pub fn index_file(path: &Path, origin: Origin) -> Option<FileSymbols> {
    let bytes = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    Some(extract_text(&text, origin == Origin::Stub))
}

struct Counters {
    parsed: AtomicUsize,
    from_cache: AtomicUsize,
    done: AtomicUsize,
}

/// Extracts every file, reusing cached declarations where the stamp or the content matches, and
/// reports finished files in batches. A cached file is not read at all: its declarations stay in the
/// cache file until something needs them. The cache is rewritten when anything in it changed.
pub fn run(
    files: Vec<(PathBuf, Origin)>,
    cache_path: Option<&Path>,
    stub_root: Option<&Path>,
    threads: usize,
    emit: &(dyn Fn(IndexEvent) + Sync),
) {
    let started = Instant::now();
    let total = files.len();
    emit(IndexEvent::Discovered(total));
    let cache = cache_path.map(Cache::load).unwrap_or_default();
    let cached_before = cache.files.len();
    let counters = Counters {
        parsed: AtomicUsize::new(0),
        from_cache: AtomicUsize::new(0),
        done: AtomicUsize::new(0),
    };
    let entries: Mutex<Vec<Entry>> = Mutex::new(Vec::with_capacity(total));

    let work = || {
        files.par_chunks(BATCH).for_each(|chunk| {
            let mut batch = Vec::with_capacity(chunk.len());
            let mut fresh = Vec::with_capacity(chunk.len());
            for (path, origin) in chunk {
                if let Some((source, summary, entry)) = process(path, *origin, &cache, &counters, cache_path.is_some())
                {
                    let extension = stub_root.and_then(|root| stubs::extension_of(root, path));
                    batch.push(IndexedFile {
                        path: path.clone(),
                        origin: *origin,
                        extension,
                        source,
                        summary,
                    });
                    if let Some(entry) = entry {
                        fresh.push(entry);
                    }
                }
            }
            counters.done.fetch_add(chunk.len(), Ordering::Relaxed);
            if let Ok(mut all) = entries.lock() {
                all.append(&mut fresh);
            }
            emit(IndexEvent::Files(batch));
        });
    };
    match rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .stack_size(64 << 20)
        .build()
    {
        Ok(pool) => pool.install(work),
        Err(_) => work(),
    }

    let entries = entries.into_inner().unwrap_or_default();
    let parsed = counters.parsed.load(Ordering::Relaxed);
    let from_cache = counters.from_cache.load(Ordering::Relaxed);
    let removed = cached_before.saturating_sub(
        entries
            .iter()
            .filter(|entry| cache.files.contains_key(&entry.path))
            .count(),
    );
    if let Some(path) = cache_path {
        if parsed > 0 || removed > 0 || cached_before == 0 {
            if let Ok(saved) = Cache::save(path, entries) {
                let moved = saved
                    .places
                    .into_iter()
                    .map(|(path, (offset, length))| {
                        (
                            PathBuf::from(path),
                            SymbolSource::Cached {
                                store: saved.store.clone(),
                                offset,
                                length,
                            },
                        )
                    })
                    .collect();
                emit(IndexEvent::Persisted(moved));
            }
        }
    }
    emit(IndexEvent::Finished(Stats {
        files: total,
        parsed,
        from_cache,
        discovery: Duration::ZERO,
        total: started.elapsed(),
    }));
}

type Processed = (SymbolSource, Arc<FileSummary>, Option<Entry>);

fn from_cached(path: &Path, cached: &CachedFile, cache: &Cache, stamp: Stamp) -> Option<Processed> {
    let store = cache.store.as_ref()?;
    let entry = Entry {
        path: path.to_string_lossy().into_owned(),
        stamp,
        hash: cached.hash,
        summary: cached.summary.clone(),
        payload: Payload::Stored {
            store: store.clone(),
            offset: cached.offset,
            length: cached.length,
        },
    };
    Some((cached.source(store), cached.summary.clone(), Some(entry)))
}

/// Reads a file's declarations from the cache, or from the file when the cache has no current
/// version. With `defer` set (there is a cache to write to) the declarations of a file read here are
/// not kept: the cache will have them, and until then they are read again when asked for.
fn process(path: &Path, origin: Origin, cache: &Cache, counters: &Counters, defer: bool) -> Option<Processed> {
    let stamp = Stamp::of(path)?;
    if stamp.size > MAX_FILE_SIZE {
        return None;
    }
    let key = path.to_string_lossy();
    let cached = cache.files.get(key.as_ref());
    if let Some(cached) = cached.filter(|cached| cached.stamp == stamp) {
        if let Some(done) = from_cached(path, cached, cache, stamp) {
            counters.from_cache.fetch_add(1, Ordering::Relaxed);
            return Some(done);
        }
    }
    let bytes = std::fs::read(path).ok()?;
    let hash = content_hash(&bytes);
    if let Some(cached) = cached.filter(|cached| cached.hash == hash) {
        if let Some(done) = from_cached(path, cached, cache, stamp) {
            counters.from_cache.fetch_add(1, Ordering::Relaxed);
            return Some(done);
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    let symbols = extract_text(&text, origin == Origin::Stub);
    counters.parsed.fetch_add(1, Ordering::Relaxed);
    let summary = Arc::new(FileSummary::of(&symbols));
    let entry = Entry::new(path, stamp, hash, summary.clone(), &symbols);
    let source = if defer {
        SymbolSource::Parse {
            path: path.to_path_buf(),
            stub: origin == Origin::Stub,
        }
    } else {
        SymbolSource::Memory(Arc::new(symbols))
    };
    Some((source, summary, entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn collect(files: Vec<(PathBuf, Origin)>, cache: Option<&Path>) -> (Vec<IndexedFile>, Stats) {
        let (indexed, stats, _) = collect_with_moves(files, cache);
        (indexed, stats)
    }

    /// Like `collect`, with the files the cache file took over at the end.
    fn collect_with_moves(
        files: Vec<(PathBuf, Origin)>,
        cache: Option<&Path>,
    ) -> (Vec<IndexedFile>, Stats, Vec<(PathBuf, SymbolSource)>) {
        let collected = Mutex::new(Vec::new());
        let moved = Mutex::new(Vec::new());
        let stats = Mutex::new(Stats::default());
        run(files, cache, None, 2, &|event| match event {
            IndexEvent::Files(mut batch) => collected.lock().expect("lock").append(&mut batch),
            IndexEvent::Finished(done) => *stats.lock().expect("lock") = done,
            IndexEvent::Persisted(mut batch) => moved.lock().expect("lock").append(&mut batch),
            IndexEvent::Discovered(_) => {}
        });
        (
            collected.into_inner().expect("lock"),
            stats.into_inner().expect("lock"),
            moved.into_inner().expect("lock"),
        )
    }

    #[test]
    fn discovers_project_files_and_skips_generated_folders() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for path in [
            "src/A.php",
            "tests/ATest.php",
            "vendor/x/y.php",
            "node_modules/z.php",
            ".git/hooks.php",
            "storage/framework/views/v.php",
            "README.md",
        ] {
            let file = root.join(path);
            fs::create_dir_all(file.parent().expect("a parent")).expect("created");
            fs::write(file, "<?php class X {}").expect("written");
        }
        let found = discover_project(root, None, &[]);
        let names: Vec<String> = found
            .iter()
            .map(|(path, _)| path.strip_prefix(root).expect("inside").to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["src/A.php", "tests/ATest.php"]);
    }

    #[test]
    fn finds_the_files_composer_loads_that_a_walk_leaves_out() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for path in ["src/A.php", "lib/legacy.inc", ".tools/helpers.php"] {
            let file = root.join(path);
            fs::create_dir_all(file.parent().expect("a parent")).expect("created");
            fs::write(file, "<?php class X {}").expect("written");
        }
        fs::create_dir_all(root.join("vendor/composer")).expect("vendor");
        fs::write(
            root.join("vendor/composer/autoload_classmap.php"),
            "<?php\nreturn array(\n    'Legacy' => $baseDir . '/lib/legacy.inc',\n    'App\\\\A' => $baseDir . '/src/A.php',\n);\n",
        )
        .expect("the class map");
        let composer = Composer::from_json(
            root,
            &serde_json::json!({ "autoload": { "files": [".tools/helpers.php"] } }),
        );
        let found = discover_project(root, Some(&composer), &[]);
        let names: Vec<(String, Origin)> = found
            .iter()
            .map(|(path, origin)| {
                (
                    path.strip_prefix(root).expect("inside").to_string_lossy().into_owned(),
                    *origin,
                )
            })
            .collect();
        assert_eq!(
            names,
            vec![
                ("src/A.php".to_string(), Origin::Project),
                (".tools/helpers.php".to_string(), Origin::Project),
                ("lib/legacy.inc".to_string(), Origin::Project),
            ]
        );
    }

    fn touch(root: &Path, relative: &str) {
        let file = root.join(relative);
        fs::create_dir_all(file.parent().expect("a parent")).expect("created");
        fs::write(file, "{}").expect("written");
    }

    #[test]
    fn finds_the_composer_roots_below_a_folder_without_one() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for path in [
            "backend/composer.json",
            "backend/packages/inner/composer.json",
            "shop/api/composer.json",
            "~backup/old/composer.json",
            "frontend/node_modules/lib/composer.json",
            "backend/vendor/acme/composer.json",
            ".hidden/composer.json",
            "a/b/c/d/e/composer.json",
        ] {
            touch(root, path);
        }
        let names: Vec<String> = discover_composer_roots(root)
            .iter()
            .map(|path| path.strip_prefix(root).expect("inside").to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["backend", "shop/api"]);

        touch(root, "composer.json");
        assert!(
            discover_composer_roots(root).is_empty(),
            "a folder with a composer.json is one project"
        );
    }

    #[test]
    fn a_project_skips_the_composer_roots_that_are_projects_of_their_own() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let root = dir.path();
        for path in ["tools/Tool.php", "backend/src/B.php"] {
            let file = root.join(path);
            fs::create_dir_all(file.parent().expect("a parent")).expect("created");
            fs::write(file, "<?php class X {}").expect("written");
        }
        let found = discover_project(root, None, &[root.join("backend")]);
        assert_eq!(found.len(), 1);
        assert!(found[0].0.ends_with("tools/Tool.php"));
    }

    #[test]
    fn a_cached_file_stays_on_disk_until_it_is_asked_for() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("a.php");
        fs::write(&path, "<?php class A { public function m(): int {} } function f() {}").expect("written");
        let files = vec![(path.clone(), Origin::Project)];
        let cache = dir.path().join("cache/p.bin");

        // The first run reads the file again when asked, until the cache file takes over.
        let (indexed, _, moved) = collect_with_moves(files.clone(), Some(&cache));
        assert!(matches!(indexed[0].source, SymbolSource::Parse { .. }));
        assert_eq!(moved.len(), 1);
        assert!(moved[0].1.is_cached());

        let (indexed, stats, moved) = collect_with_moves(files, Some(&cache));
        assert_eq!(stats.from_cache, 1);
        assert!(moved.is_empty(), "nothing changed, so the cache was not written again");
        assert!(indexed[0].source.is_cached());
        assert_eq!(indexed[0].summary.classes[0].name, "A");
        assert_eq!(indexed[0].summary.functions[0].name, "f");
        let symbols = indexed[0].source.load().expect("read from the cache file");
        assert_eq!(symbols.classes[0].methods[0].name, "m");
    }

    #[test]
    fn indexes_in_parallel_and_reuses_the_cache() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut files = Vec::new();
        for number in 0..40 {
            let path = dir.path().join(format!("f{number}.php"));
            fs::write(
                &path,
                format!("<?php class C{number} {{ public function m(): int {{}} }}"),
            )
            .expect("written");
            files.push((path, Origin::Project));
        }
        let cache = dir.path().join("cache/p.bin");
        let (indexed, stats) = collect(files.clone(), Some(&cache));
        assert_eq!(indexed.len(), 40);
        assert_eq!((stats.parsed, stats.from_cache), (40, 0));

        let (_, stats) = collect(files.clone(), Some(&cache));
        assert_eq!((stats.parsed, stats.from_cache), (0, 40));

        // Touched without a change in content: the hash saves the parse.
        let touched = &files[3].0;
        let text = fs::read_to_string(touched).expect("read");
        std::thread::sleep(Duration::from_millis(20));
        fs::write(touched, &text).expect("written");
        let (_, stats) = collect(files.clone(), Some(&cache));
        assert_eq!((stats.parsed, stats.from_cache), (0, 40));

        fs::write(touched, "<?php class Changed {}").expect("written");
        let (indexed, stats) = collect(files, Some(&cache));
        assert_eq!(stats.parsed, 1);
        assert!(
            indexed
                .iter()
                .any(|file| file.summary.classes.iter().any(|class| class.name == "Changed"))
        );
    }
}
