//! Finds the usages of every route, config key, view, translation and the like that the project's
//! PHP files name, and holds them against a plain search for the quoted string, to see what find
//! usages leaves out and why: `cargo run --release -p php-analysis --example key_usages -- <project>
//! <stubs> [--samples <n>]`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use php_analysis::context::FileContext;
use php_analysis::frameworks::keys::keys_in;
use php_analysis::references::{Current, Sources, hits_of_symbols};
use php_analysis::refs::Symbol;
use php_index::framework::keys::KeyKind;
use php_index::indexer::{self, IndexEvent};
use php_index::words::WordIndex;
use php_index::{Origin, Project, StubFile};
use php_syntax::{PhpVersion, parse};

struct Disk<'a> {
    words: &'a WordIndex,
}

impl Sources for Disk<'_> {
    fn candidates(&self, word: &str) -> Vec<PathBuf> {
        self.words.candidates(word)
    }

    fn text(&self, path: &Path) -> Option<String> {
        std::fs::read(path)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: key_usages <project> <stubs> [--samples <n>]");
        std::process::exit(2);
    }
    let samples: usize = args
        .iter()
        .position(|arg| arg == "--samples")
        .and_then(|at| args.get(at + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(10);
    let threads = std::thread::available_parallelism().map_or(4, usize::from);
    let stubs = Mutex::new(Vec::new());
    indexer::run(
        indexer::discover_stubs(Path::new(&args[1])),
        None,
        Some(Path::new(&args[1])),
        threads,
        &|event| {
            if let IndexEvent::Files(batch) = event {
                stubs
                    .lock()
                    .expect("lock")
                    .extend(batch.into_iter().map(StubFile::from_indexed));
            }
        },
    );
    let stubs = stubs.into_inner().expect("lock");
    let mut project = Project::open(Path::new(&args[0]), PhpVersion::V8_4);
    let files = indexer::discover_project(&project.root, project.composer.as_ref(), &[]);
    let collected = Mutex::new(Vec::new());
    indexer::run(files, None, None, threads, &|event| {
        if let IndexEvent::Files(batch) = event {
            collected.lock().expect("lock").extend(batch);
        }
    });
    let extensions = project.extensions();
    project.apply(collected.into_inner().expect("lock"));
    project.index.set_stubs(&stubs, &extensions);
    let index = &project.index;

    let own: Vec<PathBuf> = index
        .files()
        .filter(|file| file.origin == Origin::Project)
        .map(|file| file.path.clone())
        .collect();
    let started = Instant::now();
    let mut words = WordIndex::default();
    words.build(own.clone(), None);
    println!(
        "words of {} files in {:?}, {} hashes",
        words.len(),
        started.elapsed(),
        words.hash_count()
    );

    let mut keys: BTreeSet<(String, String)> = BTreeSet::new();
    let mut symbols: BTreeMap<(String, String), Symbol> = BTreeMap::new();
    for path in &own {
        if path.to_string_lossy().ends_with(".blade.php") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let root = parse(&text).syntax();
        let ctx = FileContext::new(index, &root);
        for key in keys_in(&ctx) {
            let id = (key.kind.label().to_string(), key.value.clone());
            if keys.insert(id.clone()) {
                symbols.insert(
                    id,
                    Symbol::Key {
                        kind: key.kind,
                        name: key.value,
                        scope: key.scope,
                    },
                );
            }
        }
    }
    let sources = Disk { words: &words };
    let texts: BTreeMap<PathBuf, String> = own
        .iter()
        .filter_map(|path| Some((path.clone(), std::fs::read_to_string(path).ok()?)))
        .collect();
    let mut per_kind: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut slowest = Duration::ZERO;
    let mut total = Duration::ZERO;
    let mut missed: Vec<String> = Vec::new();
    let empty = String::new();
    let none = parse("").syntax();
    let nowhere = PathBuf::from("/nowhere.php");
    for ((kind, name), symbol) in &symbols {
        let began = Instant::now();
        let found = hits_of_symbols(
            index,
            &sources,
            &Current {
                path: &nowhere,
                text: &empty,
                root: &none,
            },
            std::slice::from_ref(symbol),
        );
        let took = began.elapsed();
        total += took;
        slowest = slowest.max(took);
        let hits: BTreeSet<(PathBuf, usize)> = found
            .iter()
            .flat_map(|file| {
                file.hits
                    .iter()
                    .map(|hit| (file.path.clone(), usize::from(hit.range.start())))
            })
            .collect();
        let entry = per_kind.entry(kind.clone()).or_default();
        entry.0 += 1;
        entry.1 += hits.len();
        // A form field is a plain word that every array of the project may hold as a key.
        if matches!(
            symbol,
            Symbol::Key {
                kind: KeyKind::Field | KeyKind::EntityField,
                ..
            }
        ) {
            continue;
        }
        for (path, text) in &texts {
            for quote in ['\'', '"'] {
                let quoted = format!("{quote}{name}{quote}");
                for (at, _) in text.match_indices(&quoted) {
                    if !hits.contains(&(path.clone(), at + 1)) {
                        let line_start = text[..at].rfind('\n').map_or(0, |found| found + 1);
                        let line_end = text[at..].find('\n').map_or(text.len(), |found| at + found);
                        missed.push(format!(
                            "{kind} {name}: {}: {}",
                            path.strip_prefix(&project.root).unwrap_or(path).display(),
                            text[line_start..line_end].trim()
                        ));
                    }
                }
            }
        }
    }
    for (kind, (count, hits)) in &per_kind {
        println!("{kind}: {count} names, {hits} usages");
    }
    println!("{} searches in {:?}, slowest {:?}", symbols.len(), total, slowest);
    println!("{} quoted strings that are not counted as usages", missed.len());
    let step = (missed.len() / samples.max(1)).max(1);
    for line in missed.iter().step_by(step).take(samples) {
        println!("  {line}");
    }
}
