//! Finds the SQL in the strings of a project's PHP files as the server does, reads each string
//! with `sql-embed` and counts what it finds and what SQL reports, to find strings taken for SQL
//! that are none and diagnostics that are wrong: `cargo run --release -p php-analysis --example
//! sql_survey -- <project> <stubs> [--dialect <name>] [--list] [--code <code>] [--no-heuristic]`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use php_analysis::sql::{Detection, embedded, project_dialect};
use php_index::indexer::{self, IndexEvent};
use php_index::{Origin, Project, StubFile};
use php_syntax::PhpVersion;
use sql_embed::{Analysis, Dialect, Environment, Settings};

fn line_of(text: &str, offset: u32) -> usize {
    text[..offset as usize].matches('\n').count() + 1
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: sql_survey <project> <stubs> [--dialect <name>] [--list] [--code <code>] [--no-heuristic]");
        std::process::exit(2);
    }
    let option = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let list = args.iter().any(|arg| arg == "--list");
    let code = option("--code");
    let detection = Detection {
        heuristic: !args.iter().any(|arg| arg == "--no-heuristic"),
        ..Detection::default()
    };
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
    let own: Vec<std::path::PathBuf> = index
        .files()
        .filter(|file| file.origin == Origin::Project)
        .map(|file| file.path.clone())
        .collect();
    let configured = project_dialect(&project.root, index.frameworks(), &own);
    let dialect = option("--dialect")
        .and_then(|name| Dialect::parse(&name))
        .or(configured)
        .unwrap_or(Dialect::Generic);
    println!(
        "{} PHP files, the project is configured for {configured:?}, read as {dialect:?}",
        own.len()
    );
    let settings = Settings {
        dialect,
        ..Settings::default()
    };
    let env = Environment::new(settings, None, None);
    let mut reasons: BTreeMap<String, usize> = BTreeMap::new();
    let mut codes: BTreeMap<String, usize> = BTreeMap::new();
    let mut detect_time = Duration::ZERO;
    let mut analysis_time = Duration::ZERO;
    let mut slowest = (Duration::ZERO, String::new());
    let only = option("--file");
    for path in &own {
        if only.as_ref().is_some_and(|only| !path.ends_with(only)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let _document = php_analysis::document::enter(Some(path));
        let root = php_syntax::parse(&text).syntax();
        // The first file pays for the sections of the framework layer; the second run is the cost of
        // a keystroke.
        let _ = embedded(index, &root, detection);
        let started = Instant::now();
        let found = embedded(index, &root, detection);
        let took = started.elapsed();
        detect_time += took;
        if took > slowest.0 {
            slowest = (took, path.display().to_string());
        }
        for string in &found {
            *reasons
                .entry(format!("{:?} {:?}", string.reason, string.fragment.kind()))
                .or_default() += 1;
            let started = Instant::now();
            let analysis = Analysis::new(&env, &string.fragment);
            let diagnostics = analysis.diagnostics();
            analysis_time += started.elapsed();
            let line = line_of(&text, u32::from(string.range.start()));
            if list {
                println!(
                    "{}:{line}: {:?} {:?} {}",
                    path.display(),
                    string.reason,
                    string.fragment.kind(),
                    analysis.sql().replace('\n', " ").chars().take(100).collect::<String>()
                );
            }
            for diagnostic in diagnostics {
                *codes.entry(diagnostic.code.to_string()).or_default() += 1;
                if code.as_deref() == Some(diagnostic.code) {
                    println!(
                        "{}:{}: {} [{}] in {}",
                        path.display(),
                        line_of(&text, diagnostic.span.start),
                        diagnostic.message,
                        &text[diagnostic.span.start as usize..diagnostic.span.end as usize],
                        analysis.sql().replace('\n', " ").chars().take(120).collect::<String>()
                    );
                }
            }
        }
    }
    println!("\nStrings read as SQL:");
    for (reason, count) in &reasons {
        println!("  {count:6}  {reason}");
    }
    println!("\nDiagnostics:");
    for (code, count) in &codes {
        println!("  {count:6}  {code}");
    }
    println!(
        "\nFinding the strings took {} ms in all, {} ms at most for one file ({}); reading them {} ms",
        detect_time.as_millis(),
        slowest.0.as_millis(),
        slowest.1,
        analysis_time.as_millis()
    );
}
