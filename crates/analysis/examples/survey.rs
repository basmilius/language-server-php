//! Runs every inspection over the project's own files and counts what they report, to measure how
//! often they are wrong on code that is right: `cargo run --release -p php-analysis --example
//! survey -- <project> <stubs> [--code <code>] [--samples <n>] [--vendor] [--all]`. `--vendor` reads
//! the installed packages instead, `--all` also switches on the inspections that are off.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use php_analysis::inspections::{Externals, INSPECTIONS, InspectionEnv, InspectionSettings, Override, inspect};
use php_index::indexer::{self, IndexEvent};
use php_index::{Origin, Project, StubFile};
use php_syntax::{PhpVersion, parse};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: survey <project> <stubs> [--code <code>] [--samples <n>] [--vendor] [--all]");
        std::process::exit(2);
    }
    let flag_value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let only_code = flag_value("--code");
    let samples: usize = flag_value("--samples")
        .and_then(|value| value.parse().ok())
        .unwrap_or(5);
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
    let mut classes: HashSet<String> = HashSet::new();
    let mut functions: HashSet<String> = HashSet::new();
    let mut constants: HashSet<String> = HashSet::new();
    for stub in &stubs {
        classes.extend(stub.summary.classes.iter().map(|class| class.name.to_ascii_lowercase()));
        functions.extend(
            stub.summary
                .functions
                .iter()
                .map(|function| function.name.to_ascii_lowercase()),
        );
        constants.extend(stub.summary.constants.iter().map(|constant| constant.name.clone()));
    }
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

    let mut settings = InspectionSettings::default();
    if args.iter().any(|arg| arg == "--all") {
        for info in INSPECTIONS {
            settings.set(
                info.code,
                Override {
                    enabled: Some(true),
                    severity: None,
                },
            );
        }
    }
    let is_class = |name: &str| classes.contains(&name.to_ascii_lowercase());
    let is_function = |name: &str| functions.contains(&name.to_ascii_lowercase());
    let is_constant = |name: &str| constants.contains(name);
    let externals = Externals {
        class: &is_class,
        function: &is_function,
        constant: &is_constant,
    };

    let wanted = if args.iter().any(|arg| arg == "--vendor") {
        Origin::Vendor
    } else {
        Origin::Project
    };
    let mut paths: Vec<_> = project
        .index
        .files()
        .filter(|file| file.origin == wanted)
        .map(|file| file.path.clone())
        .collect();
    paths.sort();
    let started = Instant::now();
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut shown: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut lines = 0usize;
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        lines += text.lines().count();
        let tree = parse(&text);
        let root = tree.syntax();
        let env = InspectionEnv {
            index: &project.index,
            text: &text,
            root: &root,
            settings: &settings,
            ready: true,
            externals: &externals,
        };
        for finding in inspect(&env) {
            let code = finding.diagnostic.code;
            *counts.entry(code).or_default() += 1;
            if only_code.as_deref().is_some_and(|only| only != code) {
                continue;
            }
            let shown_here = shown.entry(code).or_default();
            if *shown_here >= samples {
                continue;
            }
            *shown_here += 1;
            let offset = usize::from(finding.diagnostic.range.start());
            let line = text[..offset].matches('\n').count() + 1;
            let relative = path.strip_prefix(&project.root).unwrap_or(path);
            let snippet = text.lines().nth(line - 1).unwrap_or("").trim();
            println!(
                "{code}  {}:{line}  {}\n      {snippet}",
                relative.display(),
                finding.diagnostic.message
            );
        }
    }
    println!(
        "\n{} files, {} lines, {} ms",
        paths.len(),
        lines,
        started.elapsed().as_millis()
    );
    for (code, count) in &counts {
        println!("{count:6}  {code}");
    }
    if counts.is_empty() {
        println!("nothing reported");
    }
}
