//! Asks the analyses a question about a real project: `cargo run --release -p php-analysis --example
//! probe -- <project> <stubs> <file> <line> <column> [complete|hover|definition|references|rename:<name>|tokens|hints|signature]`. Lines and columns
//! count from 1 and in bytes.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use php_analysis::Analyzer;
use php_analysis::completion::{CompletionOptions, complete};
use php_analysis::references::{Current, Sources, references_at};
use php_analysis::rename::rename;
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
    if args.len() < 5 {
        eprintln!(
            "usage: probe <project> <stubs> <file> <line> <column> [complete|hover|definition|references|rename:<name>|tokens|hints|signature]"
        );
        std::process::exit(2);
    }
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

    let text = std::fs::read_to_string(&args[2]).expect("the file reads");
    let line: usize = args[3].parse().expect("a line");
    let column: usize = args[4].parse().expect("a column");
    let offset: usize = text.split_inclusive('\n').take(line - 1).map(str::len).sum::<usize>() + column - 1;
    let mode = args.get(5).map_or("complete", String::as_str);
    let started = Instant::now();
    match mode {
        "hover" | "definition" => {
            let root = parse(&text).syntax();
            let analyzer = Analyzer::new(&project.index, &root, offset as u32);
            if mode == "hover" {
                match analyzer.hover(offset as u32) {
                    Some(hover) => println!("{}", hover.markdown),
                    None => println!("nothing"),
                }
            } else {
                for place in analyzer.definitions(offset as u32) {
                    println!("{place:?}");
                }
            }
        }
        "references" => {
            let words_started = Instant::now();
            let mut words = WordIndex::default();
            words.build(
                project
                    .index
                    .files()
                    .filter(|file| file.origin == Origin::Project)
                    .map(|file| file.path.clone())
                    .collect(),
                None,
            );
            println!("words of {} files in {:?}", words.len(), words_started.elapsed());
            let started = Instant::now();
            let root = parse(&text).syntax();
            let path = PathBuf::from(&args[2]);
            let found = references_at(
                &project.index,
                &Disk { words: &words },
                &Current {
                    path: &path,
                    text: &text,
                    root: &root,
                },
                offset as u32,
            );
            match found {
                Some(found) => {
                    println!("{:?}", found.symbols);
                    let total: usize = found.files.iter().map(|file| file.hits.len()).sum();
                    for file in found.files.iter().take(8) {
                        println!("{} {}", file.path.display(), file.hits.len());
                    }
                    println!(
                        "{total} places in {} files in {:?}",
                        found.files.len(),
                        started.elapsed()
                    );
                }
                None => println!("nothing"),
            }
            return;
        }
        "tokens" => {
            let root = parse(&text).syntax();
            let started = Instant::now();
            let tokens = php_analysis::semantic_tokens::semantic_tokens(&project.index, &root, None);
            println!("{} tokens in {:?}", tokens.len(), started.elapsed());
            for token in tokens.iter().take(15) {
                println!(
                    "{} {} {}",
                    &text[token.start as usize..token.end as usize],
                    php_analysis::semantic_tokens::TOKEN_TYPES[token.ty as usize],
                    token.modifiers
                );
            }
            return;
        }
        mode if mode.starts_with("rename:") => {
            let mut words = WordIndex::default();
            words.build(
                project
                    .index
                    .files()
                    .filter(|file| file.origin == Origin::Project)
                    .map(|file| file.path.clone())
                    .collect(),
                None,
            );
            let started = Instant::now();
            let root = parse(&text).syntax();
            let path = PathBuf::from(&args[2]);
            let result = rename(
                &project.index,
                &Disk { words: &words },
                &Current {
                    path: &path,
                    text: &text,
                    root: &root,
                },
                offset as u32,
                &mode["rename:".len()..],
            );
            match result {
                Ok(done) => {
                    let total: usize = done.files.iter().map(|file| file.edits.len()).sum();
                    println!(
                        "{total} edits in {} files, file rename {:?}, in {:?}",
                        done.files.len(),
                        done.file_rename.map(|moved| moved.to),
                        started.elapsed()
                    );
                }
                Err(message) => println!("refused: {message}"),
            }
            return;
        }
        _ => {
            let list = complete(&project.index, &text, offset as u32, CompletionOptions::default());
            for item in list.items.iter().take(25) {
                println!(
                    "{:?} {} {} | {}{}",
                    item.kind,
                    item.label,
                    item.detail.as_deref().unwrap_or(""),
                    item.description.as_deref().unwrap_or(""),
                    if item.additional_edits.is_empty() {
                        ""
                    } else {
                        " (+import)"
                    }
                );
            }
            println!(
                "{} items{} in {:?}",
                list.items.len(),
                if list.incomplete { "+" } else { "" },
                started.elapsed()
            );
            return;
        }
    }
    println!("in {:?}", started.elapsed());
}
