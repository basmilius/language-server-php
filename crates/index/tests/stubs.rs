//! The standard library stubs, read from the corpus the fetch script puts in `corpus/` (these tests
//! report that and pass when it is not there), at different language levels.

use std::path::PathBuf;
use std::sync::Mutex;

use php_index::indexer::{self, IndexEvent};
use php_index::stubs::selected_extensions;
use php_index::{Index, StubFile, Type};
use php_syntax::PhpVersion;

fn corpus() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/phpstorm-stubs");
    if root.join("standard").exists() {
        Some(root)
    } else {
        eprintln!("no corpus: run scripts/fetch-corpus.sh");
        None
    }
}

fn load(root: &std::path::Path) -> Vec<StubFile> {
    let found = Mutex::new(Vec::new());
    indexer::run(indexer::discover_stubs(root), None, Some(root), 4, &|event| {
        if let IndexEvent::Files(batch) = event {
            found
                .lock()
                .expect("lock")
                .extend(batch.into_iter().map(StubFile::from_indexed));
        }
    });
    found.into_inner().expect("lock")
}

fn index_at(level: PhpVersion, stubs: &[StubFile]) -> Index {
    let mut index = Index::new(level);
    index.set_stubs(stubs, &selected_extensions(&[]));
    index
}

#[test]
fn stubs_are_filtered_by_the_language_level() {
    let Some(root) = corpus() else {
        return;
    };
    let stubs = load(&root);
    let old = index_at(PhpVersion::V8_1, &stubs);
    let new = index_at(PhpVersion::V8_4, &stubs);

    assert!(old.function("array_find").is_none(), "array_find is new in 8.4");
    assert!(new.function("array_find").is_some());
    assert!(old.function("str_contains").is_some());
    assert!(
        old.class("Random\\Randomizer").is_none(),
        "Random\\Randomizer is new in 8.2"
    );
    assert!(new.class("Random\\Randomizer").is_some());
    assert!(old.class("Stringable").is_some());

    let legacy = index_at(PhpVersion::V7_4, &stubs);
    assert!(legacy.class("Stringable").is_none(), "Stringable is new in 8.0");
    assert!(legacy.function("str_contains").is_none());
}

#[test]
fn leveled_types_and_parameters_follow_the_level() {
    let Some(root) = corpus() else {
        return;
    };
    let stubs = load(&root);
    let modern = index_at(PhpVersion::V8_4, &stubs);
    let function = modern.function("strlen").expect("strlen");
    assert_eq!(function.decl.callable.params_at(PhpVersion::V8_4).count(), 1);

    let date = Type::class("DateTimeImmutable");
    let method = modern.find_method(&date, "format").expect("format");
    assert_eq!(
        method
            .member
            .callable
            .native_return(PhpVersion::V8_4)
            .map(|ty| ty.display(false)),
        Some("string".to_string())
    );
    // `json_decode` documents its return as `mixed`.
    assert!(modern.function("json_decode").is_some());
}

#[test]
fn the_hierarchy_of_the_standard_library_resolves() {
    let Some(root) = corpus() else {
        return;
    };
    let stubs = load(&root);
    let index = index_at(PhpVersion::V8_4, &stubs);
    assert!(index.is_subclass_of("ArrayIterator", "Traversable"));
    assert!(index.is_subclass_of("InvalidArgumentException", "Throwable"));
    assert!(index.is_subclass_of("DateTimeImmutable", "DateTimeInterface"));
    let methods = index.methods(&Type::class("ArrayObject"));
    assert!(methods.iter().any(|found| found.member.name == "count"));
}
