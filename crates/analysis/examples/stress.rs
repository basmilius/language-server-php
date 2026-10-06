//! Runs the position-based analyses at every name of every project file and reports a panic or an
//! answer that points outside its file: `cargo run --release -p php-analysis --example stress --
//! <project> <stubs> [limit of files]`. With `STRESS_VENDOR` set it reads the installed packages
//! instead of the project's own files.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use php_analysis::hierarchy::{prepare_call_hierarchy, prepare_type_hierarchy};
use php_analysis::inlay_hints::{HintOptions, inlay_hints};
use php_analysis::references::{Current, highlights_at};
use php_analysis::rename::prepare_rename;
use php_analysis::semantic_tokens::semantic_tokens;
use php_analysis::signature::signature_help;
use php_index::indexer::{self, IndexEvent};
use php_index::{Origin, Project, StubFile};
use php_syntax::SyntaxKind::*;
use php_syntax::{PhpVersion, parse};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: stress <project> <stubs> [limit of files]");
        std::process::exit(2);
    }
    let limit: usize = args.get(2).and_then(|limit| limit.parse().ok()).unwrap_or(usize::MAX);
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

    let wanted = if std::env::var("STRESS_VENDOR").is_ok() {
        Origin::Vendor
    } else {
        Origin::Project
    };
    let paths: Vec<PathBuf> = project
        .index
        .files()
        .filter(|file| file.origin == wanted)
        .map(|file| file.path.clone())
        .take(limit)
        .collect();
    let started = Instant::now();
    let (mut checked, mut failures, mut names) = (0usize, 0usize, 0usize);
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let root = parse(&text).syntax();
        let index = &project.index;
        let mut offsets: Vec<u32> = Vec::new();
        for token in root
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
        {
            if matches!(
                token.kind(),
                IDENT | VARIABLE | QUALIFIED_NAME | FULLY_QUALIFIED_NAME | LPAREN
            ) {
                offsets.push(u32::from(token.text_range().start()) + 1);
            }
        }
        names += offsets.len();
        let result = catch_unwind(AssertUnwindSafe(|| {
            let length = text.len() as u32;
            for token in semantic_tokens(index, &root, None) {
                assert!(token.end <= length && token.start < token.end, "token out of range");
            }
            for hint in inlay_hints(index, &root, None, HintOptions::default()) {
                assert!(hint.offset <= length, "hint out of range");
            }
            let current = Current {
                path,
                text: &text,
                root: &root,
            };
            for offset in &offsets {
                for hit in highlights_at(index, &php_analysis::references::NoSources, &current, *offset) {
                    assert!(u32::from(hit.range.end()) <= length, "highlight out of range");
                }
                let _ = prepare_rename(index, &root, &text, *offset);
                let _ = prepare_call_hierarchy(index, &root, *offset);
                let _ = prepare_type_hierarchy(index, &root, *offset);
                let _ = signature_help(index, &root, *offset);
            }
        }));
        checked += 1;
        if result.is_err() {
            failures += 1;
            println!("panic in {}", path.display());
        }
    }
    println!(
        "{checked} files, {names} positions, {failures} failures in {:?}",
        started.elapsed()
    );
    if failures > 0 {
        std::process::exit(1);
    }
}
