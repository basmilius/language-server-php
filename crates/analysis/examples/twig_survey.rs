//! Reads every Twig template of a project as the server does and asks hover, definition and
//! completion at a spread of offsets, to find panics, time the requests and count the functions,
//! filters and tests the extensions do not declare: `cargo run --release -p php-analysis --example
//! twig_survey -- <project> <stubs> [--every <n>]`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use php_analysis::completion::CompletionOptions;
use php_analysis::twig::{self, Template};
use php_index::framework::symfony::templates::Templates;
use php_index::framework::twig::TwigExtensions;
use php_index::indexer::{self, IndexEvent};
use php_index::{Project, StubFile};
use php_syntax::PhpVersion;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: twig_survey <project> <stubs> [--every <n>]");
        std::process::exit(2);
    }
    let every: usize = args
        .iter()
        .position(|arg| arg == "--every")
        .and_then(|at| args.get(at + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(7);
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
    let extensions = index.section::<TwigExtensions>();
    println!(
        "{} functions, filters and tests from the extensions",
        extensions.entries.len()
    );
    let templates: Vec<_> = index
        .section::<Templates>()
        .templates
        .iter()
        .map(|template| template.path.clone())
        .collect();
    let (mut calls, mut names) = (0usize, 0usize);
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let (mut requests, mut total, mut slowest, mut panics) = (0usize, Duration::ZERO, Duration::ZERO, 0usize);
    let mut read_time = Duration::ZERO;
    for path in &templates {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let began = Instant::now();
        let template = Template::read(index, Some(path), &text);
        read_time += began.elapsed();
        names += template.names.len();
        for call in &template.calls {
            calls += 1;
            if extensions.find(call.kind, &call.name).is_none() {
                *unknown.entry(format!("{:?} {}", call.kind, call.name)).or_default() += 1;
            }
        }
        for offset in (0..text.len())
            .step_by(every)
            .filter(|offset| text.is_char_boundary(*offset))
        {
            let offset = offset as u32;
            let began = Instant::now();
            let outcome = std::panic::catch_unwind(|| {
                let _ = twig::hover_at(index, Some(path), &text, offset);
                let _ = twig::definitions_at(index, Some(path), &text, offset);
                let _ = twig::complete_at(index, Some(path), &text, offset, CompletionOptions::default());
            });
            let took = began.elapsed();
            if outcome.is_err() {
                panics += 1;
                eprintln!("panic at {}:{offset}", path.display());
            }
            requests += 3;
            total += took;
            slowest = slowest.max(took);
        }
    }
    println!("{} templates read in {read_time:?}", templates.len());
    println!("{requests} requests in {total:?}, the slowest three at one offset {slowest:?}, {panics} panics");
    println!(
        "{names} names, {calls} calls of functions, filters and tests, {} not declared:",
        unknown.values().sum::<usize>()
    );
    for (call, count) in &unknown {
        println!("  {count:>4}  {call}");
    }
}
