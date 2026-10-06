use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::references::{Current, Sources};
use crate::rename::{Rename, prepare_rename, rename};
use crate::testing::{Fixture, split_cursor};

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

fn apply(text: &str, edits: &[crate::rename::TextEdit]) -> String {
    let mut out = text.to_string();
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|edit| std::cmp::Reverse(edit.range.start()));
    for edit in sorted {
        out.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.text,
        );
    }
    out
}

struct Outcome {
    result: Result<Rename, String>,
    /// The text of every file that was edited, by file name, after the edits.
    texts: HashMap<String, String>,
}

fn run(files: &[(&str, &str)], current: &str, new_name: &str) -> Outcome {
    let fixture = Fixture::new(files).with_current(current);
    let (text, root, offset) = split_cursor(current);
    let sources = Files(fixture.sources.clone());
    let path = PathBuf::from("/project/current.php");
    let result = rename(
        &fixture.index,
        &sources,
        &Current {
            path: &path,
            text: &text,
            root: &root,
        },
        offset,
        new_name,
    );
    let mut texts = HashMap::new();
    if let Ok(done) = &result {
        for file in &done.files {
            let before = if file.path == path {
                text.clone()
            } else {
                fixture.sources[&file.path].clone()
            };
            let name = file
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            texts.insert(name, apply(&before, &file.edits));
        }
    }
    Outcome { result, texts }
}

#[test]
fn renames_a_class_with_its_imports_docs_and_file() {
    let user = "<?php\nnamespace App\\Models;\n\nclass User {}\n";
    let service = "<?php\nnamespace App\\Services;\n\nuse App\\Models\\User;\nuse App\\Models\\User as Person;\n\n/** @param User $user */\nfunction f(User $user, Person $other): ?User { return new User(); }\n";
    let current = "<?php\nnamespace App\\Models;\n\nclass Admin extends Us$0er {}\n";
    let outcome = run(
        &[("Models/User.php", user), ("service.php", service)],
        current,
        "Member",
    );
    let done = outcome.result.expect("renamed");
    let moved = done.file_rename.expect("the file follows the class");
    assert_eq!(moved.from, PathBuf::from("/project/Models/User.php"));
    assert_eq!(moved.to, PathBuf::from("/project/Models/Member.php"));
    assert_eq!(moved.class, "App\\Models\\User");
    assert_eq!(
        outcome.texts["User.php"],
        "<?php\nnamespace App\\Models;\n\nclass Member {}\n"
    );
    assert_eq!(
        outcome.texts["service.php"],
        "<?php\nnamespace App\\Services;\n\nuse App\\Models\\Member;\nuse App\\Models\\Member as Person;\n\n/** @param Member $user */\nfunction f(Member $user, Person $other): ?Member { return new Member(); }\n"
    );
    assert_eq!(
        outcome.texts["current.php"],
        "<?php\nnamespace App\\Models;\n\nclass Admin extends Member {}\n"
    );
}

#[test]
fn a_class_file_that_does_not_carry_the_name_stays_put() {
    let user = "<?php\nclass User {}\nclass Other {}\n";
    let outcome = run(&[("models.php", user)], "<?php\n$u = new Us$0er();\n", "Member");
    assert!(outcome.result.expect("renamed").file_rename.is_none());
}

#[test]
fn renames_a_method_across_its_hierarchy_and_its_callers() {
    let contract = "<?php\nnamespace App;\n\ninterface Handler { public function handle(int $x): void; }\n";
    let first = "<?php\nnamespace App;\n\nclass First implements Handler { public function handle(int $x): void {} }\n";
    let other = "<?php\nnamespace App;\n\nclass Other { public function handle(int $x): void {} }\n";
    let caller = "<?php\nnamespace App;\n\nfunction run(Handler $h, First $f, Other $o) {\n    $h->handle(1);\n    $f->handle(2);\n    $o->handle(3);\n}\n";
    let current = "<?php\nnamespace App;\n\nclass Second extends First { public function han$0dle(int $x): void { parent::handle($x); } }\n";
    let outcome = run(
        &[
            ("handler.php", contract),
            ("first.php", first),
            ("other.php", other),
            ("caller.php", caller),
        ],
        current,
        "process",
    );
    outcome.result.as_ref().expect("renamed");
    assert!(outcome.texts["handler.php"].contains("function process(int $x): void;"));
    assert!(outcome.texts["first.php"].contains("function process("));
    assert!(outcome.texts["caller.php"].contains("$h->process(1);"));
    assert!(outcome.texts["caller.php"].contains("$f->process(2);"));
    assert!(
        outcome.texts["caller.php"].contains("$o->handle(3);"),
        "an unrelated class keeps its method"
    );
    assert!(!outcome.texts.contains_key("other.php"));
    assert!(outcome.texts["current.php"].contains("function process(int $x): void { parent::process($x); }"));
}

#[test]
fn refuses_a_method_name_that_is_taken_in_the_hierarchy() {
    let base = "<?php\nclass Base { public function run() {} public function stop() {} }\n";
    let current = "<?php\nclass Child extends Base { public function st$0art() {} }\n";
    let outcome = run(&[("base.php", base)], current, "stop");
    assert_eq!(outcome.result.unwrap_err(), "Base already has a method named 'stop'");
}

#[test]
fn renames_a_promoted_property_with_its_named_arguments_and_uses() {
    let point = "<?php\nnamespace App;\n\nclass Point {\n    /** @property-read int $length */\n    public function __construct(private int $x, public int $y = 0) {\n        echo $x;\n    }\n    public function sum(): int { return $this->x + $this->y; }\n}\n";
    let usage = "<?php\nnamespace App;\n\n$p = new Point(x: 1, y: 2);\necho $p->y;\n";
    let current = "<?php\nnamespace App;\n\nfunction f(Point $p) { return $p->$0y; }\n";
    let outcome = run(&[("point.php", point), ("usage.php", usage)], current, "height");
    outcome.result.as_ref().expect("renamed");
    assert_eq!(
        outcome.texts["point.php"],
        "<?php\nnamespace App;\n\nclass Point {\n    /** @property-read int $length */\n    public function __construct(private int $x, public int $height = 0) {\n        echo $x;\n    }\n    public function sum(): int { return $this->x + $this->height; }\n}\n"
    );
    assert_eq!(
        outcome.texts["usage.php"],
        "<?php\nnamespace App;\n\n$p = new Point(x: 1, height: 2);\necho $p->height;\n"
    );
}

#[test]
fn renaming_a_promoted_parameter_renames_the_property_too() {
    let point = "<?php\nclass Point {\n    public function __construct(private int $x) { echo $x; }\n    public function get(): int { return $this->x; }\n}\n";
    let usage = "<?php\n$p = new Point(x: 1);\n";
    let outcome = run(
        &[("usage.php", usage)],
        &point.replace("int $x) {", "int $$0x) {"),
        "left",
    );
    outcome.result.as_ref().expect("renamed");
    assert_eq!(
        outcome.texts["current.php"],
        "<?php\nclass Point {\n    public function __construct(private int $left) { echo $left; }\n    public function get(): int { return $this->left; }\n}\n"
    );
    assert_eq!(outcome.texts["usage.php"], "<?php\n$p = new Point(left: 1);\n");
}

#[test]
fn renames_a_parameter_with_its_doc_and_named_arguments() {
    let lib = "<?php\n/**\n * @param int $count How many\n */\nfunction repeat(string $text, int $count) { return str_repeat($text, $count); }\nrepeat(text: 'a', count: 2);\n";
    let current = "<?php\nrepeat(text: 'b', cou$0nt: 3);\n";
    let outcome = run(&[("lib.php", lib)], current, "times");
    outcome.result.as_ref().expect("renamed");
    assert_eq!(
        outcome.texts["lib.php"],
        "<?php\n/**\n * @param int $times How many\n */\nfunction repeat(string $text, int $times) { return str_repeat($text, $times); }\nrepeat(text: 'a', times: 2);\n"
    );
    assert_eq!(outcome.texts["current.php"], "<?php\nrepeat(text: 'b', times: 3);\n");
}

#[test]
fn renames_a_variable_inside_its_scope_only() {
    let current = "<?php\nfunction one($a) {\n    $f = fn($x) => $x + $a;\n    $g = function () use ($a) { return $a; };\n    return $$0a;\n}\nfunction two($a) { return $a; }\n";
    let outcome = run(&[], current, "total");
    outcome.result.as_ref().expect("renamed");
    assert_eq!(
        outcome.texts["current.php"],
        "<?php\nfunction one($total) {\n    $f = fn($x) => $x + $total;\n    $g = function () use ($total) { return $total; };\n    return $total;\n}\nfunction two($a) { return $a; }\n"
    );
}

#[test]
fn refuses_a_variable_name_already_in_the_scope() {
    let current = "<?php\nfunction one($a, $b) { return $$0a + $b; }\n";
    assert_eq!(
        run(&[], current, "b").result.unwrap_err(),
        "A variable named '$b' is already used in this scope"
    );
}

#[test]
fn renames_functions_constants_class_constants_and_enum_cases() {
    let lib = "<?php\nnamespace Lib;\n\nconst LIMIT = 10;\nfunction helper(): int { return LIMIT; }\nenum Suit { case Hearts; case Spades; }\nclass K { const MAX = 1; public function m() { return self::MAX + static::MAX; } }\n";
    let usage = "<?php\nuse function Lib\\helper;\nuse const Lib\\LIMIT;\nuse Lib\\{Suit, K};\n\necho helper() + LIMIT + K::MAX;\n$s = Suit::Hearts;\n";
    let outcome = run(
        &[("lib.php", lib)],
        &usage.replace("helper() +", "hel$0per() +"),
        "assist",
    );
    assert_eq!(
        outcome.texts["current.php"],
        "<?php\nuse function Lib\\assist;\nuse const Lib\\LIMIT;\nuse Lib\\{Suit, K};\n\necho assist() + LIMIT + K::MAX;\n$s = Suit::Hearts;\n"
    );
    assert!(outcome.texts["lib.php"].contains("function assist(): int"));

    let outcome = run(&[("lib.php", lib)], &usage.replace("K::MAX", "K::MA$0X"), "CEILING");
    assert!(outcome.texts["lib.php"].contains("const CEILING = 1;"));
    assert!(outcome.texts["lib.php"].contains("self::CEILING + static::CEILING"));
    assert!(outcome.texts["current.php"].contains("K::CEILING"));

    let outcome = run(
        &[("lib.php", lib)],
        &usage.replace("Suit::Hearts", "Suit::Hea$0rts"),
        "Wands",
    );
    assert!(outcome.texts["lib.php"].contains("case Wands; case Spades;"));
    assert!(outcome.texts["current.php"].contains("Suit::Wands"));
}

#[test]
fn validates_the_new_name() {
    let current = "<?php\nclass Fo$0o {}\n";
    assert_eq!(
        run(&[], current, "1abc").result.unwrap_err(),
        "'1abc' is not a valid name"
    );
    assert_eq!(
        run(&[], current, "class").result.unwrap_err(),
        "'class' is a reserved word"
    );
    assert_eq!(
        run(&[], current, "string").result.unwrap_err(),
        "'string' is a reserved word"
    );
    assert_eq!(
        run(&[("bar.php", "<?php\nclass Bar {}\n")], current, "Bar")
            .result
            .unwrap_err(),
        "A class named 'Bar' already exists"
    );
    let method = "<?php\nclass A { public function f$0() {} }\n";
    assert!(
        run(&[], method, "list").result.is_ok(),
        "a method may be named after a keyword"
    );
}

#[test]
fn will_not_rename_what_is_declared_elsewhere() {
    let fixture = Fixture::with_level(
        php_syntax::PhpVersion::V8_4,
        &[],
        &[("standard/basic.php", "<?php\nfunction strlen(string $string): int {}\n")],
    )
    .with_current("<?php\nstrlen('a');\n");
    let (text, root, offset) = split_cursor("<?php\nstr$0len('a');\n");
    let result = prepare_rename(&fixture.index, &root, &text, offset);
    assert_eq!(
        result.unwrap_err(),
        "This name is declared in the PHP standard library and cannot be renamed"
    );
    let (text, root, offset) = split_cursor("<?php\nclass A { public function f() { return $th$0is; } }\n");
    assert_eq!(
        prepare_rename(&fixture.index, &root, &text, offset).unwrap_err(),
        "'$this' is a keyword and cannot be renamed"
    );
    let (text, root, offset) = split_cursor("<?php\nclass A { public function f() { return sel$0f::class; } }\n");
    assert!(
        prepare_rename(&fixture.index, &root, &text, offset)
            .unwrap_err()
            .contains("keyword")
    );
}

#[test]
fn renames_a_namespace_in_declarations_imports_and_qualified_names() {
    let model = "<?php\nnamespace App\\Models;\n\nclass User {}\n";
    let service = "<?php\nnamespace App\\Services;\n\nuse App\\Models\\User;\nuse App\\Models\\{Post, Tag as T};\nuse App\\Models\\Sub\\Deep;\n\n$a = new \\App\\Models\\User();\n";
    let current = "<?php\nnamespace App\\Mod$0els;\n\nclass Post {}\n";
    let outcome = run(
        &[("user.php", model), ("service.php", service)],
        current,
        "App\\Entities",
    );
    assert_eq!(
        outcome.texts["user.php"],
        "<?php\nnamespace App\\Entities;\n\nclass User {}\n"
    );
    assert_eq!(
        outcome.texts["service.php"],
        "<?php\nnamespace App\\Services;\n\nuse App\\Entities\\User;\nuse App\\Entities\\{Post, Tag as T};\nuse App\\Models\\Sub\\Deep;\n\n$a = new \\App\\Entities\\User();\n"
    );
    assert_eq!(
        outcome.texts["current.php"],
        "<?php\nnamespace App\\Entities;\n\nclass Post {}\n"
    );
}

#[test]
fn renames_a_class_imported_through_a_group() {
    let lib = "<?php\nnamespace Lib;\n\nclass Old {}\nclass Keep {}\n";
    let usage = "<?php\nuse Lib\\{Old, Keep};\nuse Lib\\{Old as Legacy};\n\n$a = new Old();\n$b = new Legacy();\n";
    let current = "<?php\nuse Lib\\Old;\n$x = new O$0ld();\n";
    let outcome = run(&[("lib.php", lib), ("usage.php", usage)], current, "Fresh");
    assert_eq!(
        outcome.texts["usage.php"],
        "<?php\nuse Lib\\{Fresh, Keep};\nuse Lib\\{Fresh as Legacy};\n\n$a = new Fresh();\n$b = new Legacy();\n"
    );
    assert_eq!(
        outcome.texts["current.php"],
        "<?php\nuse Lib\\Fresh;\n$x = new Fresh();\n"
    );
}

#[test]
fn renaming_a_class_or_a_method_renames_the_strings_that_hold_it() {
    let home = "<?php\nnamespace App\\Http;\n\nclass Home {\n    public function show() {}\n}\n";
    let routes = "<?php\n$routes = ['App\\Http\\Home', 'App\\Http\\Home@show', 'App\\Http\\Home::show', 'Home'];\n";
    let migration = "<?php\n$type = 'App\\Http\\Home';\n";
    let outcome = run(
        &[
            ("Http/Home.php", home),
            ("routes.php", routes),
            ("database/migrations/2020_01_01_000000_types.php", migration),
        ],
        "<?php\nnew \\App\\Http\\Ho$0me();\n",
        "Start",
    );
    outcome.result.expect("renamed");
    assert!(!outcome.texts.contains_key("2020_01_01_000000_types.php"));
    assert_eq!(
        outcome.texts["routes.php"],
        "<?php\n$routes = ['App\\Http\\Start', 'App\\Http\\Start@show', 'App\\Http\\Start::show', 'Home'];\n"
    );
    let outcome = run(
        &[("Http/Home.php", home), ("routes.php", routes)],
        "<?php\n(new \\App\\Http\\Home())->sh$0ow();\n",
        "index",
    );
    outcome.result.expect("renamed");
    assert_eq!(
        outcome.texts["routes.php"],
        "<?php\n$routes = ['App\\Http\\Home', 'App\\Http\\Home@index', 'App\\Http\\Home::index', 'Home'];\n"
    );
}

#[test]
fn renames_the_named_arguments_of_an_attribute() {
    let lib = "<?php\n#[\\Attribute]\nclass Route { public function __construct(public string $path = '/') {} }\n";
    let current = "<?php\n#[Route(pa$0th: '/home')]\nfunction home() {}\n";
    let outcome = run(&[("lib.php", lib)], current, "uri");
    outcome.result.as_ref().expect("renamed");
    assert_eq!(
        outcome.texts["lib.php"],
        "<?php\n#[\\Attribute]\nclass Route { public function __construct(public string $uri = '/') {} }\n"
    );
    assert_eq!(
        outcome.texts["current.php"],
        "<?php\n#[Route(uri: '/home')]\nfunction home() {}\n"
    );
}
