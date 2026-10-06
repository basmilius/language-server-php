//! Reads every Blade template of a project as the server does and asks hover, definition and
//! completion at a spread of offsets, to find panics, time the requests and count the variables the
//! type layer cannot type: `cargo run --release -p php-analysis --example blade_survey -- <project>
//! <stubs> [--every <n>]`. With `BLADE_AT=<template>:<offset>` it times the requests at that one place.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use php_analysis::blade::{self, Template};
use php_analysis::completion::CompletionOptions;
use php_analysis::infer::Analyzer;
use php_analysis::references::Sources;
use php_index::indexer::{self, IndexEvent};
use php_index::words::WordIndex;
use php_index::{Origin, Project, StubFile, Type};
use php_syntax::PhpVersion;
use php_syntax::SyntaxKind::VARIABLE;

struct Disk<'a> {
    words: &'a WordIndex,
}

impl Sources for Disk<'_> {
    fn candidates(&self, word: &str) -> Vec<std::path::PathBuf> {
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
        eprintln!("usage: blade_survey <project> <stubs> [--every <n>]");
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

    if let Ok(spot) = std::env::var("BLADE_AT") {
        let (file, offset) = spot.rsplit_once(':').expect("file:offset");
        let path = Path::new(file);
        let text = std::fs::read_to_string(path).expect("the template");
        let offset: u32 = offset.parse().expect("an offset");
        for round in 0..2 {
            let began = Instant::now();
            let _ = Template::read(index, Some(path), &text, &[]);
            let read = began.elapsed();
            let began = Instant::now();
            let _ = blade::hover_at(index, Some(path), &text, &[], offset);
            let hover = began.elapsed();
            let began = Instant::now();
            let _ = blade::definitions_at(index, Some(path), &text, &[], offset);
            let definition = began.elapsed();
            let began = Instant::now();
            let _ = blade::complete_at(index, Some(path), &text, &[], offset, CompletionOptions::default());
            let completion = began.elapsed();
            println!(
                "round {round}: read {read:?}, hover {hover:?}, definition {definition:?}, completion {completion:?}"
            );
        }
        return;
    }
    let mut words = WordIndex::default();
    words.build(
        index
            .files()
            .filter(|file| file.origin == Origin::Project)
            .map(|file| file.path.clone())
            .collect(),
        None,
    );
    let sources = Disk { words: &words };
    if let Ok(file) = std::env::var("BLADE_VIRTUAL") {
        let text = std::fs::read_to_string(&file).expect("the template");
        println!(
            "{}",
            Template::read(index, Some(Path::new(&file)), &text, &[]).virt.text
        );
        return;
    }
    if let Ok(file) = std::env::var("BLADE_GIVEN") {
        let began = Instant::now();
        for (name, ty) in blade::data::given(index, &sources, Path::new(&file)) {
            println!("${name}: {}", ty.display(true));
        }
        println!("in {:?}", began.elapsed());
        return;
    }
    let templates: Vec<_> = index
        .files()
        .filter(|file| blade::is_template(&file.path))
        .map(|file| file.path.clone())
        .collect();
    let (mut requests, mut slowest, mut total) = (0usize, Duration::ZERO, Duration::ZERO);
    let (mut variables, mut untyped, mut untyped_alone) = (0usize, 0usize, 0usize);
    let (mut given_time, mut given_slowest, mut given_count) = (Duration::ZERO, Duration::ZERO, 0usize);
    let mut panics = 0;
    let mut findings = 0;
    let (mut diagnostics_time, mut diagnostics_slowest) = (Duration::ZERO, Duration::ZERO);
    let mut read_time = Duration::ZERO;
    let mut largest = (0usize, Duration::ZERO);
    let mut livewire = (0usize, 0usize, 0usize, 0usize);
    for path in &templates {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let began = Instant::now();
        let given = blade::data::given(index, &sources, path);
        let took = began.elapsed();
        given_time += took;
        given_slowest = given_slowest.max(took);
        given_count += given.len();
        let alone = Template::read(index, Some(path), &text, &[]);
        let began = Instant::now();
        let template = Template::read(index, Some(path), &text, &given);
        let took = began.elapsed();
        read_time += took;
        if text.lines().count() > largest.0 {
            largest = (text.lines().count(), took);
        }
        let began = Instant::now();
        let reported = blade::diagnostics(
            index,
            Some(path),
            &text,
            &given,
            &php_analysis::inspections::InspectionSettings::default(),
            !path.starts_with(project.root.join("vendor")),
        );
        let took = began.elapsed();
        diagnostics_time += took;
        diagnostics_slowest = diagnostics_slowest.max(took);
        if took > Duration::from_millis(20) {
            eprintln!("slow: diagnostics {took:?} at {}", path.display());
        }
        for found in reported {
            findings += 1;
            let line = text[..usize::from(found.range.start())].matches('\n').count() + 1;
            println!(
                "{}  {}:{line}  {}",
                found.code,
                path.strip_prefix(&project.root).unwrap_or(path).display(),
                found.message
            );
        }
        for name in template
            .names
            .iter()
            .filter(|name| name.kind == php_index::framework::keys::KeyKind::Livewire)
        {
            livewire.0 += 1;
            if !php_index::framework::keys::definitions(index, name.kind, &name.value, None).is_empty() {
                livewire.1 += 1;
            } else if std::env::var("BLADE_LIVEWIRE").is_ok() {
                eprintln!("unresolved <livewire:{}> in {}", name.value, path.display());
            }
        }
        for wire in blade::livewire::wire_refs(index, Some(path), &text) {
            livewire.2 += 1;
            if !blade::definitions_at(index, Some(path), &text, &[], u32::from(wire.range.start())).is_empty() {
                livewire.3 += 1;
            } else if std::env::var("BLADE_LIVEWIRE").is_ok() {
                eprintln!("unresolved wire {} in {}", wire.name, path.display());
            }
        }
        let (counted, missing) = untyped_variables(index, &template);
        variables += counted;
        untyped += missing;
        untyped_alone += untyped_variables(index, &alone).1;
        for offset in (0..text.len())
            .step_by(every)
            .filter(|offset| text.is_char_boundary(*offset))
        {
            let offset = offset as u32;
            let began = Instant::now();
            let outcome = std::panic::catch_unwind(|| {
                let _ = blade::hover_at(index, Some(path), &text, &[], offset);
                let _ = blade::definitions_at(index, Some(path), &text, &[], offset);
                let _ = blade::complete_at(index, Some(path), &text, &[], offset, CompletionOptions::default());
            });
            let took = began.elapsed();
            if took > Duration::from_millis(40) {
                eprintln!("slow: {:?} at {}:{offset}", took, path.display());
            }
            if outcome.is_err() {
                panics += 1;
                eprintln!("panic at {}:{offset}", path.display());
            }
            requests += 3;
            total += took;
            slowest = slowest.max(took);
        }
    }
    println!(
        "{} templates read in {read_time:?}, the longest ({} lines) in {:?}",
        templates.len(),
        largest.0,
        largest.1
    );
    println!("{requests} requests in {total:?}, the slowest three at one offset {slowest:?}, {panics} panics");
    println!("{findings} findings, diagnostics in {diagnostics_time:?}, the slowest template {diagnostics_slowest:?}");
    println!("given: {given_count} variables in {given_time:?}, the slowest template {given_slowest:?}");
    println!("{variables} variables, {untyped} without a type, {untyped_alone} without what the template is given");
    println!(
        "Livewire: {} of {} component tags and {} of {} wire names resolve",
        livewire.1, livewire.0, livewire.3, livewire.2
    );
}

/// The variables of a template's PHP, and how many of them the type layer cannot type.
fn untyped_variables(index: &php_index::Index, template: &Template) -> (usize, usize) {
    let root = template.root();
    let (mut variables, mut untyped) = (0, 0);
    for token in root
        .descendants_with_tokens()
        .filter_map(|element| element.into_token())
        .filter(|token| token.kind() == VARIABLE)
    {
        if !template.virt.is_copied(token.text_range()) {
            continue;
        }
        let Some(node) = token.parent() else {
            continue;
        };
        variables += 1;
        let analyzer = Analyzer::new(index, &root, u32::from(token.text_range().start()));
        let env = analyzer.env_around(&node);
        if matches!(analyzer.type_of(&node, &env), Type::Unknown | Type::Mixed) {
            untyped += 1;
            if std::env::var("BLADE_UNTYPED").is_ok() {
                eprintln!("untyped {}", token.text());
            }
        }
    }
    (variables, untyped)
}
