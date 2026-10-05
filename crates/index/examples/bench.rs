//! Indexes a project and prints how long it took: `cargo run --release -p php-index --example bench --
//! <project> <stubs folder> [cache folder]`. A second run with the same cache folder is the warm one.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

use php_index::indexer::{self, IndexEvent};
use php_index::{Project, StubFile};
use php_syntax::PhpVersion;

/// The resident size of this process in MB, as `ps` reports it.
fn resident_mb() -> u64 {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok();
    output
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|text| text.trim().parse::<u64>().ok())
        .map_or(0, |kilobytes| kilobytes / 1024)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(project_root), Some(stubs_root)) = (args.first(), args.get(1)) else {
        eprintln!("usage: bench <project> <stubs> [cache]");
        std::process::exit(2);
    };
    let cache = args.get(2).map(PathBuf::from);
    let threads = std::thread::available_parallelism().map_or(4, usize::from);

    let started = Instant::now();
    let stub_files = indexer::discover_stubs(std::path::Path::new(stubs_root));
    let stubs = Mutex::new(Vec::new());
    let stub_places = Mutex::new(Vec::new());
    let stub_cache = cache.as_ref().map(|dir| dir.join("stubs.bin"));
    indexer::run(
        stub_files,
        stub_cache.as_deref(),
        Some(std::path::Path::new(stubs_root)),
        threads,
        &|event| match event {
            IndexEvent::Files(batch) => stubs
                .lock()
                .expect("lock")
                .extend(batch.into_iter().map(StubFile::from_indexed)),
            IndexEvent::Persisted(moved) => stub_places.lock().expect("lock").extend(moved),
            _ => {}
        },
    );
    let mut stubs = stubs.into_inner().expect("lock");
    for (path, source) in stub_places.into_inner().expect("lock") {
        if let Some(stub) = stubs.iter_mut().find(|stub| stub.path == path) {
            stub.source = source;
        }
    }
    println!("stubs: {} files in {:?}", stubs.len(), started.elapsed());

    let mut project = Project::open(std::path::Path::new(project_root), PhpVersion::V8_4);
    println!(
        "level {} (from composer: {})",
        project.level, project.level_from_composer
    );
    let started = Instant::now();
    let files = indexer::discover_project(&project.root, project.composer.as_ref(), &[]);
    println!("discovered {} files in {:?}", files.len(), started.elapsed());
    let started = Instant::now();
    let collected = Mutex::new(Vec::new());
    let persisted = Mutex::new(Vec::new());
    let stats = Mutex::new(None);
    let cache_path = cache.as_ref().map(|dir| project.cache_path(dir));
    indexer::run(files, cache_path.as_deref(), None, threads, &|event| match event {
        IndexEvent::Files(batch) => collected.lock().expect("lock").extend(batch),
        IndexEvent::Finished(done) => *stats.lock().expect("lock") = Some(done),
        IndexEvent::Persisted(moved) => persisted.lock().expect("lock").extend(moved),
        IndexEvent::Discovered(_) => {}
    });
    let elapsed = started.elapsed();
    let extensions = project.extensions();
    project.apply(collected.into_inner().expect("lock"));
    for (path, source) in persisted.into_inner().expect("lock") {
        project.index.move_to_cache(&path, source);
    }
    project.index.set_stubs(&stubs, &extensions);
    let stats = stats.into_inner().expect("lock").expect("finished");
    println!(
        "indexed in {:?}: {} parsed, {} from cache; {} files, {} classes, {} functions",
        elapsed,
        stats.parsed,
        stats.from_cache,
        project.index.file_count(),
        project.index.class_count(),
        project.index.function_count()
    );
    println!("resident: {} MB", resident_mb());
}
