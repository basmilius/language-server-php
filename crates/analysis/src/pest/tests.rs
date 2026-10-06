use std::path::PathBuf;

use crate::completion::{CompletionOptions, complete};
use crate::infer::Analyzer;
use crate::testing::{Fixture, PEST, PHPUNIT, split_cursor};

const EVENT: &str = r#"<?php
namespace App;
final class Event {
    public string $name = '';
    public ?Venue $venue = null;
    public function startsOn(): \DateTimeImmutable {}
}
final class Venue { public string $city = ''; }
"#;

fn fixture(current: &str) -> Fixture {
    Fixture::new(&[("pest.php", PEST), ("phpunit.php", PHPUNIT), ("Event.php", EVENT)]).with_current(current)
}

fn this_type(code: &str, expression: &str) -> String {
    let fixture = fixture(code);
    let (_, root, offset) = split_cursor(code);
    let _guard = crate::document::enter(Some(&PathBuf::from("/project/tests/Feature/FooTest.php")));
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let env = analyzer.env_at(offset);
    let parsed = php_syntax::parse(&format!("<?php {expression};")).syntax();
    let node = parsed
        .descendants()
        .find(|node| node.kind() == php_syntax::SyntaxKind::EXPR_STATEMENT)
        .and_then(|statement| statement.children().next())
        .expect("an expression");
    analyzer.type_of(&node, &env).display(true)
}

#[test]
fn this_is_the_test_case_in_a_test_closure() {
    let code = r#"<?php
use App\Event;
it('works', function () {
    $0
});
"#;
    assert_eq!(this_type(code, "$this"), "static");
    let analyzer_class = {
        let fixture = fixture(code);
        let (_, root, offset) = split_cursor(code);
        Analyzer::new(&fixture.index, &root, offset).this_type().display(false)
    };
    assert_eq!(analyzer_class, "PHPUnit\\Framework\\TestCase");
}

#[test]
fn describe_and_the_top_of_the_file_have_no_test_case() {
    let code = "<?php\ndescribe('group', function () {\n    $0\n});\n";
    let fixture = fixture(code);
    let (_, root, offset) = split_cursor(code);
    assert_eq!(
        Analyzer::new(&fixture.index, &root, offset).this_type().display(false),
        "mixed"
    );
}

#[test]
fn properties_set_in_before_each_are_known_in_the_tests_of_the_file() {
    let code = r#"<?php
use App\Event;
use App\Venue;
beforeEach(function () {
    $this->event = new Event();
    $this->venue = $this->event->venue ?? new Venue();
});
it('works', function () {
    $0
});
"#;
    assert_eq!(this_type(code, "$this->event"), "Event");
    assert_eq!(this_type(code, "$this->event->venue"), "?Venue");
    assert_eq!(this_type(code, "$this->event->startsOn()"), "DateTimeImmutable");
    assert_eq!(this_type(code, "$this->venue"), "Venue");
    assert_eq!(this_type(code, "$this->unknown"), "mixed");
}

#[test]
fn a_before_each_inside_describe_only_reaches_its_own_tests() {
    let code = r#"<?php
use App\Event;
describe('inner', function () {
    beforeEach(function () { $this->event = new Event(); });
    it('sees it', function () { $0 });
});
it('does not', function () { $this->event; });
"#;
    assert_eq!(this_type(code, "$this->event"), "Event");
    let outside = r#"<?php
use App\Event;
describe('inner', function () {
    beforeEach(function () { $this->event = new Event(); });
});
it('does not', function () { $0 });
"#;
    assert_eq!(this_type(outside, "$this->event"), "mixed");
}

#[test]
fn the_case_class_and_traits_come_from_uses_for_the_folder() {
    let pest_php = "<?php\nnamespace Tests;\nuse Tests\\Support\\AppCase;\nuse Tests\\Support\\Refreshes;\nuses(AppCase::class, Refreshes::class)->in('Feature');\nuses(Other::class)->in('Unit');\n";
    let support = "<?php\nnamespace Tests\\Support;\nclass AppCase extends \\PHPUnit\\Framework\\TestCase { public function login(): void {} }\ntrait Refreshes { public function refreshDatabase(): bool {} }\n";
    let code = "<?php\nit('works', function () {\n    $0\n});\n";
    let fixture = Fixture::new(&[
        ("pest.php", PEST),
        ("phpunit.php", PHPUNIT),
        ("tests/Pest.php", pest_php),
        ("tests/Support/Support.php", support),
    ])
    .with_current(code);
    let (_, root, offset) = split_cursor(code);
    let _guard = crate::document::enter(Some(&PathBuf::from("/project/tests/Feature/Http/FooTest.php")));
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    assert_eq!(
        analyzer.this_type().display(false),
        "Tests\\Support\\AppCase&Tests\\Support\\Refreshes"
    );
    let env = analyzer.env_at(offset);
    let parsed = php_syntax::parse("<?php $this->refreshDatabase();").syntax();
    let call = parsed
        .descendants()
        .find(|node| node.kind() == php_syntax::SyntaxKind::CALL_EXPR)
        .unwrap();
    assert_eq!(analyzer.type_of(&call, &env).display(true), "bool");
    drop(_guard);
    let _unit = crate::document::enter(Some(&PathBuf::from("/project/tests/Unit/FooTest.php")));
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    assert_eq!(analyzer.this_type().display(false), "Tests\\Other");
}

#[test]
fn completion_offers_the_properties_before_each_sets() {
    let code = r#"<?php
use App\Event;
beforeEach(function () {
    $this->event = new Event();
});
it('works', function () {
    $this->ev$0
});
"#;
    let fixture = fixture(code);
    let (offset, text) = lsc_text::testing::cursor(code);
    let labels: Vec<String> = complete(&fixture.index, &text, offset, CompletionOptions::default())
        .items
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert_eq!(labels, vec!["event"]);
}

const DATASETS: &str = "<?php\ndataset('event sizes', [\n    'small' => [1, 'a'],\n    'large' => [10, 'b'],\n]);\ndataset('numbers', fn () => [1, 2, 3]);\n";

fn dataset_fixture(current: &str) -> Fixture {
    Fixture::on_disk(
        &[
            ("pest.php", PEST),
            ("phpunit.php", PHPUNIT),
            ("tests/Datasets/Sizes.php", DATASETS),
            ("tests/Feature/FooTest.php", current),
        ],
        &[],
    )
}

fn definitions(code: &str) -> Vec<String> {
    let clean = code.replacen("$0", "", 1);
    let fixture = dataset_fixture(&clean);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    analyzer
        .definitions(offset)
        .into_iter()
        .map(|place| {
            let path = place.path.unwrap();
            let source = &fixture.sources[&path];
            format!(
                "{} {}",
                path.file_name().unwrap().to_string_lossy(),
                &source[place.span.start as usize..place.span.end as usize]
            )
        })
        .collect()
}

#[test]
fn a_dataset_name_leads_to_its_declaration() {
    let code = "<?php\nit('works', function (int $size) {})->with('event si$0zes');\n";
    assert_eq!(definitions(code), vec!["Sizes.php event sizes"]);
    let unknown = "<?php\nit('works', function () {})->with('mi$0ssing');\n";
    assert_eq!(definitions(unknown), Vec::<String>::new());
}

#[test]
fn usages_and_renames_of_a_dataset_cover_its_declaration_and_every_with() {
    use crate::references::{Current, references_at};
    use crate::testing::Files;
    let code = "<?php\nit('a', function () {})->with('numbers');\nit('b', function () {})->with('num$0bers');\n";
    let clean = code.replacen("$0", "", 1);
    let fixture = dataset_fixture(&clean);
    let (text, root, offset) = split_cursor(code);
    let path = fixture
        .index
        .files()
        .find(|file| file.path.ends_with("FooTest.php"))
        .unwrap()
        .path
        .clone();
    let sources = Files(fixture.sources.clone());
    let found = references_at(
        &fixture.index,
        &sources,
        &Current {
            path: &path,
            text: &text,
            root: &root,
        },
        offset,
    )
    .expect("references");
    let mut places: Vec<String> = found
        .files
        .iter()
        .flat_map(|file| {
            let source = &fixture.sources[&file.path];
            file.hits
                .iter()
                .map(|hit| {
                    format!(
                        "{}:{}",
                        file.path.file_name().unwrap().to_string_lossy(),
                        &source[usize::from(hit.range.start())..usize::from(hit.range.end())]
                    )
                })
                .collect::<Vec<_>>()
        })
        .collect();
    places.sort();
    assert_eq!(
        places,
        vec!["FooTest.php:numbers", "FooTest.php:numbers", "Sizes.php:numbers"]
    );

    let done = crate::rename::rename(
        &fixture.index,
        &sources,
        &Current {
            path: &path,
            text: &text,
            root: &root,
        },
        offset,
        "counts",
    )
    .expect("renamed");
    assert_eq!(done.files.iter().map(|file| file.edits.len()).sum::<usize>(), 3);
}

#[test]
fn dataset_names_complete_inside_with() {
    let code = "<?php\ndataset('own', [1]);\nit('a', function () {})->with('$0');\n";
    let clean = code.replacen("$0", "", 1);
    let fixture = dataset_fixture(&clean);
    let offset = code.find("$0").unwrap() as u32;
    let labels: Vec<String> = complete(&fixture.index, &clean, offset, CompletionOptions::default())
        .items
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert_eq!(labels, vec!["event sizes", "numbers", "own"]);
    let declaring = "<?php\ndataset('$0', [1]);\n";
    let clean = declaring.replacen("$0", "", 1);
    let fixture = dataset_fixture(&clean);
    let offset = declaring.find("$0").unwrap() as u32;
    assert!(
        complete(&fixture.index, &clean, offset, CompletionOptions::default())
            .items
            .is_empty()
    );
}

fn param_types(code: &str, names: &[&str]) -> Vec<String> {
    let clean = code.replacen("$0", "", 1);
    let fixture = dataset_fixture(&clean);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let env = analyzer.env_at(offset);
    names
        .iter()
        .map(|name| {
            env.get(name)
                .map_or_else(|| "<unset>".to_string(), |ty| ty.display(true))
        })
        .collect()
}

#[test]
fn an_inline_dataset_types_the_parameters_of_the_test() {
    let code =
        "<?php\nit('works', function ($count, $label, $flag) { $0 })->with([[1, 'a', true], [2, 'b', false]]);\n";
    assert_eq!(
        param_types(code, &["count", "label", "flag"]),
        vec!["int", "string", "bool"]
    );
    let keyed = "<?php\nit('works', function ($count, $label) { $0 })->with(['one' => [1, 'a'], 'two' => [2, 'b']]);\n";
    assert_eq!(param_types(keyed, &["count", "label"]), vec!["int", "string"]);
    let single = "<?php\nit('works', function ($number) { $0 })->with([1, 2, 3]);\n";
    assert_eq!(param_types(single, &["number"]), vec!["int"]);
}

#[test]
fn a_named_dataset_types_the_parameters_where_it_is_declared_in_the_project() {
    let code = "<?php\nit('works', function ($count, $label) { $0 })->with('event sizes');\n";
    assert_eq!(param_types(code, &["count", "label"]), vec!["int", "string"]);
    let numbers = "<?php\nit('works', function ($n) { $0 })->with('numbers');\n";
    assert_eq!(param_types(numbers, &["n"]), vec!["int"]);
}

#[test]
fn a_dataset_that_cannot_be_read_gives_nothing() {
    let code = "<?php\nit('works', function ($a) { $0 })->with(range(1, 3));\n";
    assert_eq!(param_types(code, &["a"]), vec!["mixed"]);
    let mixed = "<?php\nit('works', function ($a, $b) { $0 })->with([[1, 2], 3]);\n";
    assert_eq!(param_types(mixed, &["a", "b"]), vec!["mixed", "mixed"]);
    let two = "<?php\nit('works', function ($a) { $0 })->with([1])->with([2]);\n";
    assert_eq!(param_types(two, &["a"]), vec!["mixed"]);
}

#[test]
fn expectation_chains_keep_following_what_pest_declares() {
    let code = r#"<?php
use App\Event;
use App\Venue;
it('works', function () {
    $event = new Event();
    $same = expect($event)->toBe($event);
    $other = expect($event)->toBeInstanceOf(Event::class)->and(new Venue());
    $opposite = expect($event)->not->toBe(1);
    $each = expect([1, 2])->each;
    $0
});
"#;
    assert_eq!(this_type(code, "$event"), "Event");
    assert_eq!(this_type(code, "$same->value"), "?Event");
    assert_eq!(this_type(code, "$other->value"), "Venue");
    assert_eq!(this_type(code, "$opposite"), "Expectation");
    assert_eq!(this_type(code, "$each"), "EachExpectation");
}

const CUSTOM: &str =
    "<?php\nexpect()->extend('toBeFoo', function (string $expected) {\n    return $this->value === $expected;\n});\n";

#[test]
fn custom_expectations_are_offered_called_and_found() {
    let code = "<?php\nit('works', function () {\n    expect(1)->toBeF$0;\n});\n";
    let clean = code.replacen("$0", "", 1);
    let fixture =
        Fixture::new(&[("pest.php", PEST), ("phpunit.php", PHPUNIT), ("tests/Pest.php", CUSTOM)]).with_current(&clean);
    let offset = code.find("$0").unwrap() as u32;
    let labels: Vec<String> = complete(&fixture.index, &clean, offset, CompletionOptions::default())
        .items
        .into_iter()
        .map(|item| item.label)
        .collect();
    assert_eq!(labels, vec!["toBeFoo"]);

    let call = "<?php\nit('works', function () {\n    $r = expect(1)->toBeFoo('x');\n    $0\n});\n";
    let clean = call.replacen("$0", "", 1);
    let fixture =
        Fixture::new(&[("pest.php", PEST), ("phpunit.php", PHPUNIT), ("tests/Pest.php", CUSTOM)]).with_current(&clean);
    let (_, root, offset) = split_cursor(call);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    assert_eq!(
        analyzer.env_at(offset).get("r").map(|ty| ty.display(true)),
        Some("Expectation<?int>".to_string())
    );

    let named = "<?php\nit('works', function () {\n    expect(1)->toBeF$0oo('x');\n});\n";
    let clean = named.replacen("$0", "", 1);
    let fixture =
        Fixture::new(&[("pest.php", PEST), ("phpunit.php", PHPUNIT), ("tests/Pest.php", CUSTOM)]).with_current(&clean);
    let (_, root, offset) = split_cursor(named);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let places = analyzer.definitions(offset);
    assert_eq!(places.len(), 1);
    let source = &fixture.sources[places[0].path.as_ref().unwrap()];
    assert_eq!(
        &source[places[0].span.start as usize..places[0].span.end as usize],
        "toBeFoo"
    );
}

#[test]
fn this_is_the_expectation_inside_a_custom_expectation() {
    let code = "<?php\nexpect()->extend('toBeFoo', function (string $expected) {\n    $0\n});\n";
    let fixture = fixture(code);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    assert_eq!(analyzer.this_type().display(false), "Pest\\Expectation<mixed>");
}
