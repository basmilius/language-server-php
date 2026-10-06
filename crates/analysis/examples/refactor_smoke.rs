//! Applies every refactor at a sample of positions of a real project and holds each result to two
//! promises: it parses as well as before, and the inspections report nothing they did not report
//! before. `cargo run --release -p php-analysis --example refactor_smoke -- <project> <stubs>
//! [--every <n>] [--per-file <n>] [--max-files <n>] [--title <prefix>[,<prefix>...]] [--show <n>]`.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use php_analysis::inspections::{Externals, InspectionEnv, InspectionSettings, inspect};
use php_analysis::refactor::{Change, RefactorEnv, with_refactors};
use php_analysis::references::Sources;
use php_format::FormatOptions;
use php_index::extract::{ExtractOptions, extract};
use php_index::indexer::{self, IndexEvent};
use php_index::{Origin, Project, StubFile};
use php_syntax::{PhpVersion, SyntaxKind, SyntaxNode, TextRange, parse};

struct DiskSources<'a> {
    words: &'a php_index::words::WordIndex,
}

impl Sources for DiskSources<'_> {
    fn candidates(&self, word: &str) -> Vec<PathBuf> {
        self.words.candidates(word)
    }

    fn text(&self, path: &Path) -> Option<String> {
        std::fs::read(path)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

#[derive(Default)]
struct Stats {
    offered: usize,
    applied: usize,
    refused: BTreeMap<String, usize>,
    parse_broken: usize,
    new_findings: usize,
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|at| args.get(at + 1))
        .cloned()
}

fn read(path: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
}

fn apply(text: &str, change: &php_analysis::refactor::FileChange) -> String {
    let mut out = String::new();
    let mut at = 0usize;
    for edit in &change.edits {
        out.push_str(&text[at..edit.start as usize]);
        out.push_str(&edit.text);
        at = edit.end as usize;
    }
    out.push_str(&text[at..]);
    out
}

fn candidates_of(root: &SyntaxNode, text: &str, budget: usize) -> Vec<TextRange> {
    let mut all: Vec<TextRange> = Vec::new();
    for node in root.descendants() {
        let range = node.text_range();
        if range.is_empty() {
            continue;
        }
        let kind = node.kind();
        let interesting = matches!(
            kind,
            SyntaxKind::EXPR_STATEMENT
                | SyntaxKind::RETURN_STATEMENT
                | SyntaxKind::ECHO_STATEMENT
                | SyntaxKind::IF_STATEMENT
                | SyntaxKind::SWITCH_STATEMENT
                | SyntaxKind::FOREACH_STATEMENT
                | SyntaxKind::WHILE_STATEMENT
                | SyntaxKind::FOR_STATEMENT
                | SyntaxKind::METHOD_DECLARATION
                | SyntaxKind::FUNCTION_DECLARATION
                | SyntaxKind::CLASS_DECLARATION
                | SyntaxKind::PROPERTY_DECLARATION
                | SyntaxKind::CLASS_CONST_DECLARATION
                | SyntaxKind::PARAMETER
                | SyntaxKind::ARGUMENT
                | SyntaxKind::ARRAY_EXPR
                | SyntaxKind::BINARY_EXPR
                | SyntaxKind::TERNARY_EXPR
                | SyntaxKind::CALL_EXPR
                | SyntaxKind::NEW_EXPR
                | SyntaxKind::INTERPOLATED_STRING
                | SyntaxKind::LITERAL
                | SyntaxKind::PROPERTY_FETCH_EXPR
                | SyntaxKind::VARIABLE_EXPR
                | SyntaxKind::ASSIGN_EXPR
        );
        if !interesting {
            continue;
        }
        let start = usize::from(range.start());
        let first_char = text[start..].chars().next().map_or(1, char::len_utf8);
        all.push(TextRange::empty(((start + first_char.min(1)) as u32).into()));
        if matches!(
            kind,
            SyntaxKind::METHOD_DECLARATION | SyntaxKind::FUNCTION_DECLARATION | SyntaxKind::CLASS_DECLARATION
        ) {
            if let Some(name) = node.children().find(|child| child.kind() == SyntaxKind::NAME) {
                all.push(TextRange::empty(name.text_range().start()));
            }
        } else if !matches!(kind, SyntaxKind::CLASS_DECLARATION) {
            all.push(range);
        }
        if matches!(kind, SyntaxKind::BLOCK) {
            continue;
        }
    }
    for node in root.descendants() {
        if matches!(node.kind(), SyntaxKind::BLOCK | SyntaxKind::STATEMENT_LIST) {
            let statements: Vec<SyntaxNode> = node.children().collect();
            for window in statements.windows(2) {
                all.push(TextRange::new(
                    window[0].text_range().start(),
                    window[1].text_range().end(),
                ));
            }
            for window in statements.windows(3) {
                all.push(TextRange::new(
                    window[0].text_range().start(),
                    window[2].text_range().end(),
                ));
            }
        }
    }
    if all.len() <= budget {
        return all;
    }
    let stride = all.len() as f64 / budget as f64;
    (0..budget).map(|i| all[(i as f64 * stride) as usize]).collect()
}

fn findings(
    index: &php_index::Index,
    text: &str,
    settings: &InspectionSettings,
    externals: &Externals<'_>,
) -> Vec<String> {
    let tree = parse(text);
    let root = tree.syntax();
    let env = InspectionEnv {
        index,
        text,
        root: &root,
        settings,
        ready: true,
        externals,
    };
    let mut out: Vec<String> = inspect(&env)
        .into_iter()
        .map(|found| format!("{} {}", found.diagnostic.code, found.diagnostic.message))
        .collect();
    out.extend(tree.errors().iter().map(|error| format!("syntax {}", error.message)));
    out.sort();
    out
}

/// A finding with the namespaces left out of the class names in its message, which a move changes.
fn shorten_names(text: &str) -> String {
    let mut out = String::new();
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        out.push_str(word.rsplit('\\').next().unwrap_or(""));
        word.clear();
    };
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '\\' {
            word.push(c);
        } else {
            flush(&mut word, &mut out);
            out.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn new_ones(before: &[String], after: &[String]) -> Vec<String> {
    let mut remaining: Vec<&String> = before.iter().collect();
    let mut new = Vec::new();
    for item in after {
        match remaining.iter().position(|old| *old == item) {
            Some(at) => {
                remaining.remove(at);
            }
            None => new.push(item.clone()),
        }
    }
    new
}

/// The edits of a change, for reading what went wrong.
fn dump(args: &[String], change: &Change, range: TextRange) -> String {
    if !args.iter().any(|arg| arg == "--dump") {
        return String::new();
    }
    let mut out = format!("\n  selection {range:?}");
    for file in &change.files {
        for edit in &file.edits {
            let text: String = edit.text.chars().take(1200).collect();
            out.push_str(&format!(
                "\n  {} {}..{} =>\n{text}\n  ----",
                file.path.display(),
                edit.start,
                edit.end
            ));
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!(
            "usage: refactor_smoke <project> <stubs> [--every <n>] [--per-file <n>] [--max-files <n>] [--title <prefix>[,<prefix>...]] [--show <n>]"
        );
        std::process::exit(2);
    }
    let every: usize = flag(&args, "--every").and_then(|v| v.parse().ok()).unwrap_or(7);
    let per_file: usize = flag(&args, "--per-file").and_then(|v| v.parse().ok()).unwrap_or(40);
    let max_files: usize = flag(&args, "--max-files").and_then(|v| v.parse().ok()).unwrap_or(150);
    let only_title = flag(&args, "--title");
    let show: usize = flag(&args, "--show").and_then(|v| v.parse().ok()).unwrap_or(15);
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
    let mut names: (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ) = Default::default();
    for stub in &stubs {
        names
            .0
            .extend(stub.summary.classes.iter().map(|class| class.name.to_ascii_lowercase()));
        names.1.extend(
            stub.summary
                .functions
                .iter()
                .map(|function| function.name.to_ascii_lowercase()),
        );
        names
            .2
            .extend(stub.summary.constants.iter().map(|constant| constant.name.clone()));
    }
    let mut project = Project::open(Path::new(&args[0]), PhpVersion::V8_4);
    let discovered = indexer::discover_project(&project.root, project.composer.as_ref(), &[]);
    let collected = Mutex::new(Vec::new());
    indexer::run(discovered, None, None, threads, &|event| {
        if let IndexEvent::Files(batch) = event {
            collected.lock().expect("lock").extend(batch);
        }
    });
    let extensions = project.extensions();
    project.apply(collected.into_inner().expect("lock"));
    project.index.set_stubs(&stubs, &extensions);
    let mut paths: Vec<PathBuf> = project
        .index
        .files()
        .filter(|file| file.origin == Origin::Project)
        .map(|file| file.path.clone())
        .collect();
    paths.sort();
    project.words.build(paths.clone(), None);
    let settings = InspectionSettings::default();
    let is_class = |name: &str| names.0.contains(&name.to_ascii_lowercase());
    let is_function = |name: &str| names.1.contains(&name.to_ascii_lowercase());
    let is_constant = |name: &str| names.2.contains(name);
    let externals = Externals {
        class: &is_class,
        function: &is_function,
        constant: &is_constant,
    };
    let sample: Vec<PathBuf> = paths.iter().step_by(every.max(1)).take(max_files).cloned().collect();
    let started = Instant::now();
    let mut stats: BTreeMap<String, Stats> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let mut before_cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for path in &sample {
        let text = read(path);
        if !parse(&text).errors().is_empty() {
            continue;
        }
        let tree = parse(&text);
        let root = tree.syntax();
        let ranges = candidates_of(&root, &text, per_file);
        for range in ranges {
            let results: Vec<(String, Result<Change, String>)> = {
                let env = InspectionEnv {
                    index: &project.index,
                    text: &text,
                    root: &root,
                    settings: &settings,
                    ready: true,
                    externals: &externals,
                };
                let sources = DiskSources { words: &project.words };
                let renv = RefactorEnv {
                    env: &env,
                    path,
                    sources: &sources,
                    composer: project.composer.as_ref(),
                    format: FormatOptions::default(),
                };
                with_refactors(&renv, range, |list| {
                    list.iter()
                        .filter(|refactor| {
                            only_title.as_ref().is_none_or(|prefixes| {
                                prefixes.split(',').any(|prefix| refactor.title.starts_with(prefix))
                            })
                        })
                        .map(|refactor| (refactor.title.clone(), refactor.run()))
                        .collect()
                })
            };
            for (title, result) in results {
                let entry = stats.entry(title.clone()).or_default();
                entry.offered += 1;
                let change = match result {
                    Ok(change) => change,
                    Err(reason) => {
                        *entry.refused.entry(reason).or_default() += 1;
                        continue;
                    }
                };
                entry.applied += 1;
                let line = text[..usize::from(range.start())].matches('\n').count() + 1;
                let mut originals: Vec<(PathBuf, String, String)> = Vec::new();
                let mut inspect_paths: Vec<(PathBuf, PathBuf)> = Vec::new();
                for file in &change.files {
                    let before = read(&file.path);
                    let after = apply(&before, file);
                    let target = change
                        .moves
                        .iter()
                        .find(|moved| moved.from == file.path)
                        .map_or(file.path.clone(), |moved| moved.to.clone());
                    inspect_paths.push((file.path.clone(), target));
                    originals.push((file.path.clone(), before, after));
                }
                for moved in &change.moves {
                    if !inspect_paths.iter().any(|(from, _)| *from == moved.from) {
                        let before = read(&moved.from);
                        originals.push((moved.from.clone(), before.clone(), before));
                        inspect_paths.push((moved.from.clone(), moved.to.clone()));
                    }
                }
                let broken: Vec<String> = originals
                    .iter()
                    .filter(|(_, before, after)| parse(after).errors().len() > parse(before).errors().len())
                    .map(|(file, _, _)| file.display().to_string())
                    .collect();
                if !broken.is_empty() {
                    if let Some(entry) = stats.get_mut(&title) {
                        entry.parse_broken += 1;
                    }
                    if failures.len() < show {
                        failures.push(format!(
                            "PARSE {title} {}:{line}: {broken:?}{}",
                            path.display(),
                            dump(&args, &change, range)
                        ));
                    }
                    continue;
                }
                for (file, _, _) in &originals {
                    before_cache
                        .entry(file.clone())
                        .or_insert_with(|| findings(&project.index, &read(file), &settings, &externals));
                }
                for (file, _, after) in &originals {
                    let symbols = Arc::new(extract(&parse(after).syntax(), ExtractOptions::default()));
                    let target = inspect_paths
                        .iter()
                        .find(|(from, _)| from == file)
                        .map_or(file.clone(), |(_, to)| to.clone());
                    if target != *file {
                        project.index.remove_file(file);
                    }
                    project.index.set_file(target, Origin::Project, symbols);
                }
                let mut new_findings: Vec<String> = Vec::new();
                for (file, _, after) in &originals {
                    let target = inspect_paths
                        .iter()
                        .find(|(from, _)| from == file)
                        .map_or(file.clone(), |(_, to)| to.clone());
                    let after_findings = findings(&project.index, after, &settings, &externals);
                    let before = before_cache.get(file).cloned().unwrap_or_default();
                    let moves_names = title.starts_with("Move class") || title.starts_with("Change the class");
                    let (before, after_findings) = if moves_names {
                        (
                            before.iter().map(|item| shorten_names(item)).collect(),
                            after_findings.iter().map(|item| shorten_names(item)).collect(),
                        )
                    } else {
                        (before, after_findings)
                    };
                    for item in new_ones(&before, &after_findings) {
                        if title == "Add parameter" && item.starts_with("unused-parameter Parameter '$parameter'") {
                            continue;
                        }
                        new_findings.push(format!("{}: {item}", target.display()));
                    }
                }
                for (file, before, _) in &originals {
                    let target = inspect_paths
                        .iter()
                        .find(|(from, _)| from == file)
                        .map_or(file.clone(), |(_, to)| to.clone());
                    if target != *file {
                        project.index.remove_file(&target);
                    }
                    let symbols = Arc::new(extract(&parse(before).syntax(), ExtractOptions::default()));
                    project.index.set_file(file.clone(), Origin::Project, symbols);
                }
                if !new_findings.is_empty() {
                    if let Some(entry) = stats.get_mut(&title) {
                        entry.new_findings += 1;
                    }
                    if failures.len() < show {
                        failures.push(format!(
                            "FINDINGS {title} {}:{line}: {new_findings:?}{}",
                            path.display(),
                            dump(&args, &change, range)
                        ));
                    }
                }
            }
        }
    }
    println!("{} files, {} ms", sample.len(), started.elapsed().as_millis());
    for (title, entry) in &stats {
        println!(
            "{title}: offered {}, applied {}, refused {}, parse errors {}, new findings {}",
            entry.offered,
            entry.applied,
            entry.refused.values().sum::<usize>(),
            entry.parse_broken,
            entry.new_findings
        );
        let mut reasons: Vec<(&String, &usize)> = entry.refused.iter().collect();
        reasons.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        for (reason, count) in reasons.into_iter().take(4) {
            println!("    {count:4}  {reason}");
        }
    }
    for failure in failures {
        println!("{failure}");
    }
}
