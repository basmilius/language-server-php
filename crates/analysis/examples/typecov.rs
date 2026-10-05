//! Counts how many expressions of the project's own files the type layer cannot type, to measure what
//! a change to it gains: `cargo run --release -p php-analysis --example typecov -- <project> <stubs>
//! [--under <folder>] [--samples <n>]`. An expression is typed when its type is not unknown or
//! `mixed`. "Receivers" are the expressions a member is read from, which is where a type is used.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use php_analysis::context::FileContext;
use php_index::indexer::{self, IndexEvent};
use php_index::{Origin, Project, StubFile, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{PhpVersion, SyntaxNode, parse};

#[derive(Default)]
struct Tally {
    typed: usize,
    unknown: usize,
    mixed: usize,
}

impl Tally {
    fn add(&mut self, ty: &Type) {
        match ty {
            Type::Mixed => self.mixed += 1,
            ty if ty.is_unknown() => self.unknown += 1,
            _ => self.typed += 1,
        }
    }

    fn line(&self, label: &str) -> String {
        let total = self.typed + self.unknown + self.mixed;
        format!(
            "{label:<12} {total:7} total {:7} typed {:7} unknown {:7} mixed ({:.1}% unresolved)",
            self.typed,
            self.unknown,
            self.mixed,
            100.0 * (self.unknown + self.mixed) as f64 / total.max(1) as f64
        )
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: typecov <project> <stubs> [--under <folder>] [--samples <n>]");
        std::process::exit(2);
    }
    let flag = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    let under = flag("--under");
    let samples: usize = flag("--samples").and_then(|value| value.parse().ok()).unwrap_or(0);
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

    let mut paths: Vec<_> = project
        .index
        .files()
        .filter(|file| file.origin == Origin::Project)
        .map(|file| file.path.clone())
        .filter(|path| {
            under.as_ref().is_none_or(|under| {
                path.strip_prefix(&project.root)
                    .is_ok_and(|relative| relative.starts_with(under))
            })
        })
        .collect();
    paths.sort();
    let mut all = Tally::default();
    let mut receivers = Tally::default();
    let mut by_kind: BTreeMap<&'static str, Tally> = BTreeMap::new();
    let mut shown = 0usize;
    let top: usize = flag("--top").and_then(|value| value.parse().ok()).unwrap_or(0);
    let mut culprits: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
    for path in &paths {
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let root = parse(&text).syntax();
        let context = FileContext::new(&project.index, &root);
        for node in root.descendants() {
            let Some(kind) = measured(&node) else {
                continue;
            };
            let analyzer = context.analyzer(&node);
            let env = analyzer.env_around(&node);
            let ty = analyzer.type_of(&node, &env);
            all.add(&ty);
            by_kind.entry(kind).or_default().add(&ty);
            if top > 0 && (ty.is_unknown() || ty == Type::Mixed) {
                *culprits.entry((kind, culprit(&node))).or_default() += 1;
            }
            if is_receiver(&node) {
                receivers.add(&ty);
                if shown < samples && (ty.is_unknown() || ty == Type::Mixed) {
                    shown += 1;
                    let offset = usize::from(node.text_range().start());
                    let line = text[..offset].matches('\n').count() + 1;
                    let relative = path.strip_prefix(&project.root).unwrap_or(path);
                    let snippet: String = node.text().to_string().chars().take(80).collect();
                    println!("{}:{line}  {}", relative.display(), snippet.replace('\n', " "));
                }
            }
        }
    }
    println!("{} files", paths.len());
    println!("{}", all.line("expressions"));
    println!("{}", receivers.line("receivers"));
    for (kind, tally) in &by_kind {
        println!("{}", tally.line(kind));
    }
    let mut ranked: Vec<_> = culprits.into_iter().collect();
    ranked.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    for ((kind, what), count) in ranked.into_iter().take(top) {
        println!("{count:6}  {kind:<9} {what}");
    }
}

/// What names an unresolved expression: the callee of a call, the member of a fetch, the variable.
fn culprit(node: &SyntaxNode) -> String {
    let text = match node.kind() {
        CALL_EXPR => node.children().next().map(|callee| callee.text().to_string()),
        _ => Some(node.text().to_string()),
    };
    let text = text.unwrap_or_default();
    let text = text.split_whitespace().collect::<Vec<_>>().join("");
    text.chars().take(60).collect()
}

/// The expressions that have a type worth counting, and a label for each.
fn measured(node: &SyntaxNode) -> Option<&'static str> {
    let parent = node.parent();
    let is_callee = parent
        .as_ref()
        .is_some_and(|parent| parent.kind() == CALL_EXPR && parent.children().next().as_ref() == Some(node));
    match node.kind() {
        VARIABLE_EXPR => {
            let text = node.text().to_string();
            let is_target = parent.as_ref().is_some_and(|parent| {
                matches!(
                    parent.kind(),
                    ASSIGN_EXPR | FOREACH_STATEMENT | ARRAY_ITEM | LIST_EXPR | ARRAY_EXPR | GLOBAL_STATEMENT
                ) && parent.children().next().as_ref() == Some(node)
                    || matches!(
                        parent.kind(),
                        FOREACH_STATEMENT | ARRAY_ITEM | LIST_EXPR | GLOBAL_STATEMENT | UNSET_STATEMENT
                    ) && parent.children().next().as_ref() != Some(node)
            });
            (text != "$this" && !is_target).then_some("variable")
        }
        CALL_EXPR if !php_analysis::infer::is_first_class_callable(node) => Some("call"),
        PROPERTY_FETCH_EXPR if !is_callee => Some("property"),
        INDEX_EXPR => Some("index"),
        SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR if !is_callee => Some("static"),
        _ => None,
    }
}

fn is_receiver(node: &SyntaxNode) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(parent.kind(), PROPERTY_FETCH_EXPR | INDEX_EXPR) && parent.children().next().as_ref() == Some(node)
    })
}
