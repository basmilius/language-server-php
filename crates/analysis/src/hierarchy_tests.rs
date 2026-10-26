use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::hierarchy::{
    CallSymbol, incoming_calls, outgoing_calls, prepare_call_hierarchy, prepare_type_hierarchy, subtypes, supertypes,
};
use crate::references::Sources;
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

const SHAPES: &str = "<?php\nnamespace App;\n\ninterface Shape {}\ntrait Colored {}\nclass Base implements Shape { use Colored; }\nclass Circle extends Base {}\nclass Square extends Base {}\n";

#[test]
fn walks_a_class_up_and_down() {
    let fixture = Fixture::new(&[("shapes.php", SHAPES)]);
    let names = |items: Vec<crate::hierarchy::TypeItem>| items.into_iter().map(|item| item.name).collect::<Vec<_>>();

    assert_eq!(
        names(supertypes(&fixture.index, "App\\Base")),
        ["App\\Shape", "App\\Colored"]
    );

    assert_eq!(names(supertypes(&fixture.index, "App\\Circle")), ["App\\Base"]);

    assert_eq!(
        names(subtypes(&fixture.index, "App\\Base")),
        ["App\\Circle", "App\\Square"]
    );

    assert_eq!(names(subtypes(&fixture.index, "App\\Shape")), ["App\\Base"]);
    let (_, root, offset) = split_cursor("<?php\nnamespace App;\n$x = new Cir$0cle();\n");
    let fixture = Fixture::new(&[("shapes.php", SHAPES)]).with_current("<?php\nnamespace App;\n$x = new Circle();\n");

    assert_eq!(
        names(prepare_type_hierarchy(&fixture.index, &root, offset)),
        ["App\\Circle"]
    );
}

const SERVICE: &str = "<?php\nnamespace App;\n\nclass Mailer {\n    public function send(): bool { return true; }\n    public function __construct() {}\n}\nfunction helper(): void {}\n";
const USER: &str = "<?php\nnamespace App;\n\nclass Notifier {\n    public function run(Mailer $mailer): void {\n        $mailer->send();\n        $mailer->send();\n        helper();\n        $x = new Mailer();\n    }\n}\nfunction top(Mailer $m) { $m->send(); }\n$m = new Mailer();\n(new Mailer())->send();\n";

#[test]
fn finds_the_callers_of_a_method_by_function_and_in_top_level_code() {
    let fixture = Fixture::new(&[("service.php", SERVICE), ("user.php", USER)]);
    let sources = Files(fixture.sources.clone());

    let calls = incoming_calls(
        &fixture.index,
        &sources,
        &CallSymbol::Method {
            class: "App\\Mailer".to_string(),
            name: "send".to_string(),
        },
    );

    let summary: Vec<(String, usize)> = calls
        .iter()
        .map(|call| {
            let name = match &call.from.symbol {
                CallSymbol::Method { class, name } => format!("{class}::{name}"),
                CallSymbol::Function(name) => name.clone(),
                CallSymbol::File(path) => format!("file {}", path.file_name().unwrap().to_string_lossy()),
                CallSymbol::Closure { label, .. } => label.clone(),
            };

            (name, call.ranges.len())
        })
        .collect();

    assert_eq!(
        summary,
        [
            ("file user.php".to_string(), 1),
            ("App\\Notifier::run".to_string(), 2),
            ("App\\top".to_string(), 1),
        ]
    );
}

#[test]
fn constructor_calls_are_found_through_new() {
    let fixture = Fixture::new(&[("service.php", SERVICE), ("user.php", USER)]);
    let sources = Files(fixture.sources.clone());

    let calls = incoming_calls(
        &fixture.index,
        &sources,
        &CallSymbol::Method {
            class: "App\\Mailer".to_string(),
            name: "__construct".to_string(),
        },
    );

    let total: usize = calls.iter().map(|call| call.ranges.len()).sum();
    assert_eq!(total, 3, "{calls:?}");
}

#[test]
fn lists_what_a_function_calls() {
    let fixture = Fixture::new(&[("service.php", SERVICE), ("user.php", USER)]);

    let item = prepare_call_hierarchy(
        &fixture.index,
        &php_syntax::parse(USER).syntax(),
        USER.find("run").unwrap() as u32 + 1,
    )
    .pop()
    .expect("an item for run");

    assert_eq!(
        item.symbol,
        CallSymbol::Method {
            class: "App\\Notifier".to_string(),
            name: "run".to_string()
        }
    );

    let calls = outgoing_calls(&fixture.index, USER, &item);

    let summary: Vec<(String, usize)> = calls
        .iter()
        .map(|call| {
            let name = match &call.to.symbol {
                CallSymbol::Method { class, name } => format!("{class}::{name}"),
                CallSymbol::Function(name) => name.clone(),
                CallSymbol::Closure { .. } | CallSymbol::File(_) => String::new(),
            };

            (name, call.ranges.len())
        })
        .collect();

    assert_eq!(
        summary,
        [
            ("App\\Mailer::send".to_string(), 2),
            ("App\\helper".to_string(), 1),
            ("App\\Mailer::__construct".to_string(), 1),
        ]
    );
}

#[test]
fn prepares_with_the_caret_touching_the_end_of_a_name() {
    let template = "<?php\nnamespace App;\n\nclass Auth {\n    public static function createSession(int $user): void {}\n}\nfunction helper(): void {}\nAuth::createSession(1);\nhelper();\n";

    let cases = [
        ("createSession(int", "declaration of a method"),
        ("createSession(1)", "static call"),
        ("helper()", "declaration of a function"),
        ("helper();\n", "call"),
    ];

    for (needle, label) in cases {
        let start = template.find(needle).expect(label);
        let name_end = start + needle.find('(').or_else(|| needle.find(';')).expect(label);

        for (offset, place) in [(name_end - 2, "inside"), (name_end, "after")] {
            let root = php_syntax::parse(template).syntax();
            let fixture = Fixture::new(&[("auth.php", template)]);
            let items = prepare_call_hierarchy(&fixture.index, &root, offset as u32);
            assert_eq!(items.len(), 1, "{label}, caret {place} the name");
        }
    }
}

const PEST: &str = "<?php\nnamespace App;\n\nit('sends a mail.', function (): void {\n    (new Mailer())->send();\n    helper();\n});\n\ntest('twice', fn () => (new Mailer())->send());\n";

#[test]
fn a_closure_that_belongs_to_no_function_is_a_caller_of_its_own() {
    let fixture = Fixture::new(&[("service.php", SERVICE), ("pest.php", PEST)]);
    let sources = Files(fixture.sources.clone());

    let calls = incoming_calls(
        &fixture.index,
        &sources,
        &CallSymbol::Method {
            class: "App\\Mailer".to_string(),
            name: "send".to_string(),
        },
    );

    let summary: Vec<(String, usize)> = calls
        .iter()
        .map(|call| match &call.from.symbol {
            CallSymbol::Closure { label, .. } => (label.clone(), call.ranges.len()),
            other => (format!("{other:?}"), call.ranges.len()),
        })
        .collect();

    assert_eq!(
        summary,
        [("it('sends a mail.')".to_string(), 1), ("test('twice')".to_string(), 1)]
    );

    let closure = &calls[0].from;

    assert_eq!(
        &PEST[closure.span.start as usize..closure.span.end as usize],
        "function (): void {\n    (new Mailer())->send();\n    helper();\n}"
    );
}

#[test]
fn lists_what_a_closure_calls() {
    let fixture = Fixture::new(&[("service.php", SERVICE), ("pest.php", PEST)]);
    let sources = Files(fixture.sources.clone());

    let calls = incoming_calls(
        &fixture.index,
        &sources,
        &CallSymbol::Method {
            class: "App\\Mailer".to_string(),
            name: "send".to_string(),
        },
    );

    let symbol = calls[0].from.symbol.clone();
    let item = crate::hierarchy::call_item(&fixture.index, &symbol).expect("item");
    let outgoing = outgoing_calls(&fixture.index, PEST, &item);
    let names: Vec<String> = outgoing.iter().map(|call| format!("{:?}", call.to.symbol)).collect();
    assert_eq!(names.len(), 3, "{names:?}");
}
