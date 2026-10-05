//! `cargo bench -p php-analysis --bench features`: semantic tokens, inlay hints, signature help,
//! usages and rename over an index of the standard library stubs and a small project made of the
//! stubs themselves. Needs the corpus from `scripts/fetch-corpus.sh`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use php_analysis::inlay_hints::{HintOptions, inlay_hints};
use php_analysis::references::{Current, Sources, references_at};
use php_analysis::rename::rename;
use php_analysis::semantic_tokens::semantic_tokens;
use php_analysis::signature::signature_help;
use php_index::indexer::{discover_stubs, extract_text};
use php_index::{Index, Origin};
use php_syntax::{PhpVersion, parse};

struct Files(HashMap<PathBuf, String>);

impl Sources for Files {
    fn candidates(&self, word: &str) -> Vec<PathBuf> {
        self.0
            .iter()
            .filter(|(_, text)| text.to_ascii_lowercase().contains(word))
            .map(|(path, _)| path.clone())
            .collect()
    }

    fn text(&self, path: &Path) -> Option<String> {
        self.0.get(path).cloned()
    }
}

fn bench(criterion: &mut Criterion) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/phpstorm-stubs");
    if !root.join("standard").exists() {
        eprintln!("no corpus: run scripts/fetch-corpus.sh");
        return;
    }
    let mut index = Index::new(PhpVersion::V8_4);
    let mut project: HashMap<PathBuf, String> = HashMap::new();
    for (path, _) in discover_stubs(&root) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            // The stubs stand in for the project's own files, so every search has files to read.
            index.set_file(path.clone(), Origin::Project, Arc::new(extract_text(&text, true)));
            project.insert(path, text);
        }
    }
    let sources = Files(project);
    let current_path = root.join("standard/standard_0.php");
    let text = std::fs::read_to_string(&current_path).unwrap_or_default();
    let tree = parse(&text).syntax();
    criterion.bench_function("semantic tokens of a stub file", |bencher| {
        bencher.iter(|| semantic_tokens(&index, &tree, None).len());
    });
    criterion.bench_function("inlay hints of a stub file", |bencher| {
        bencher.iter(|| inlay_hints(&index, &tree, None, HintOptions::default()).len());
    });
    let call = "<?php\n$parts = explode(',', $text, 2);\n";
    let call_tree = parse(call).syntax();
    let offset = call.find("', $text").expect("a marker") as u32 + 1;
    criterion.bench_function("signature help in a call", |bencher| {
        bencher.iter(|| signature_help(&index, &call_tree, offset).is_some());
    });
    let usage = "<?php\nfunction f(\\DateTimeInterface $date) { return $date->format('c'); }\n";
    let usage_tree = parse(usage).syntax();
    let usage_offset = usage.find("DateTimeInterface").expect("a marker") as u32 + 3;
    let usage_path = root.join("current.php");
    criterion.bench_function("find usages of a class over the stubs", |bencher| {
        bencher.iter(|| {
            references_at(
                &index,
                &sources,
                &Current {
                    path: &usage_path,
                    text: usage,
                    root: &usage_tree,
                },
                usage_offset,
            )
            .map(|found| found.files.len())
        });
    });
    criterion.bench_function("rename a class over the stubs", |bencher| {
        bencher.iter(|| {
            rename(
                &index,
                &sources,
                &Current {
                    path: &usage_path,
                    text: usage,
                    root: &usage_tree,
                },
                usage_offset,
                "Moment",
            )
            .map(|done| done.files.len())
            .is_ok()
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
