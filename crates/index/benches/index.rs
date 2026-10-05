//! `cargo bench -p php-index`: extracting declarations from the stubs, and building an index from them.
//! Needs the corpus from `scripts/fetch-corpus.sh`.

use std::path::PathBuf;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use php_index::indexer::{discover_stubs, extract_text};
use php_index::{Index, Origin};
use php_syntax::PhpVersion;

fn bench(criterion: &mut Criterion) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/phpstorm-stubs");
    if !root.join("standard").exists() {
        eprintln!("no corpus: run scripts/fetch-corpus.sh");
        return;
    }
    let texts: Vec<(PathBuf, String)> = discover_stubs(&root)
        .into_iter()
        .filter_map(|(path, _)| std::fs::read_to_string(&path).ok().map(|text| (path, text)))
        .collect();
    let bytes: usize = texts.iter().map(|(_, text)| text.len()).sum();
    criterion.bench_function(
        &format!("extract {} stub files ({} KB)", texts.len(), bytes / 1024),
        |bencher| {
            bencher.iter(|| {
                texts
                    .iter()
                    .map(|(_, text)| extract_text(text, true).classes.len())
                    .sum::<usize>()
            });
        },
    );
    let symbols: Vec<(PathBuf, Arc<_>)> = texts
        .iter()
        .map(|(path, text)| (path.clone(), Arc::new(extract_text(text, true))))
        .collect();
    criterion.bench_function("add the stubs to an index", |bencher| {
        bencher.iter(|| {
            let mut index = Index::new(PhpVersion::V8_4);
            for (path, symbols) in &symbols {
                index.set_file(path.clone(), Origin::Stub, symbols.clone());
            }
            index.class_count()
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
