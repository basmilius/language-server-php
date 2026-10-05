//! `cargo bench -p php-analysis`: completion and hover over an index of the standard library stubs.
//! Needs the corpus from `scripts/fetch-corpus.sh`.

use std::path::PathBuf;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use php_analysis::Analyzer;
use php_analysis::completion::{CompletionOptions, complete};
use php_index::indexer::{discover_stubs, extract_text};
use php_index::{Index, Origin};
use php_syntax::{PhpVersion, parse};

fn bench(criterion: &mut Criterion) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/phpstorm-stubs");
    if !root.join("standard").exists() {
        eprintln!("no corpus: run scripts/fetch-corpus.sh");
        return;
    }
    let mut index = Index::new(PhpVersion::V8_4);
    for (path, _) in discover_stubs(&root) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            index.set_file(path, Origin::Stub, Arc::new(extract_text(&text, true)));
        }
    }
    let text = "<?php\nnamespace App;\n\nfunction f(\\DateTimeImmutable $date, array $items) {\n    $copy = $date->modify('+1 day');\n    $copy->\n}\n";
    let member_offset = text.find("$copy->\n").expect("a marker") as u32 + 7;
    criterion.bench_function("complete members of an inferred type", |bencher| {
        bencher.iter(|| {
            complete(&index, text, member_offset, CompletionOptions::default())
                .items
                .len()
        });
    });
    let class_text = "<?php\nnamespace App;\n\nfunction g() {\n    $x = new Date\n}\n";
    let class_offset = class_text.find("Date\n").expect("a marker") as u32 + 4;
    criterion.bench_function("complete class names with an import", |bencher| {
        bencher.iter(|| {
            complete(&index, class_text, class_offset, CompletionOptions::default())
                .items
                .len()
        });
    });
    let tree = parse(text).syntax();
    let hover_offset = text.find("modify").expect("a marker") as u32 + 2;
    criterion.bench_function("hover on a method", |bencher| {
        bencher.iter(|| Analyzer::new(&index, &tree, hover_offset).hover(hover_offset).is_some());
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
