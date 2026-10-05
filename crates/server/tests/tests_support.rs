//! PHPUnit and Pest through the protocol: what a test file can run, the strings that name data
//! providers and datasets, and `$this` in a Pest closure.

mod common;

use common::*;
use serde_json::{Value, json};

const TEST_CASE: &str = "<?php\nnamespace PHPUnit\\Framework;\nabstract class TestCase {\n    public function assertTrue(mixed $condition): void {}\n}\n";

const FOO_TEST: &str = "<?php\nnamespace Tests\\Unit;\n\nuse PHPUnit\\Framework\\Attributes\\DataProvider;\nuse PHPUnit\\Framework\\TestCase;\n\nfinal class FooTest extends TestCase\n{\n    #[DataProvider('numbers')]\n    public function testIt(int $a): void {}\n\n    public static function numbers(): iterable { return [[1], [2]]; }\n}\n";

fn project() -> Disk {
    let disk = Disk::new();
    disk.write("project/phpunit.xml", "<phpunit/>");
    disk.write("project/tests/Support/PHPUnitTestCase.php", TEST_CASE);
    disk.write("project/tests/Unit/FooTest.php", FOO_TEST);
    disk
}

#[test]
fn lists_what_a_phpunit_file_can_run_as_a_request_and_as_code_lenses() {
    let disk = project();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/tests/Unit/FooTest.php");
    client.open(&uri, FOO_TEST);
    let listed = client.request("php/runnables", json!({ "uri": uri }));
    let listed = listed.as_array().expect("a list");
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert_eq!(listed[0]["kind"], "phpunit");
    assert_eq!(listed[0]["label"], "FooTest");
    assert_eq!(listed[0]["scope"], "class");
    assert_eq!(listed[0]["filter"], "/^Tests\\\\Unit\\\\FooTest::/");
    assert_eq!(listed[0]["range"]["start"], json!({ "line": 6, "character": 12 }));
    assert_eq!(listed[1]["label"], "FooTest::testIt");
    assert_eq!(
        listed[1]["filter"],
        "/^Tests\\\\Unit\\\\FooTest::testIt( with data set .*)?$/"
    );
    assert_eq!(
        listed[1]["file"],
        disk.path("project/tests/Unit/FooTest.php").to_str().unwrap()
    );
    assert_eq!(
        listed[1]["configFile"],
        disk.path("project/phpunit.xml").to_str().unwrap()
    );

    let lenses = client.request("textDocument/codeLens", json!({ "textDocument": { "uri": uri } }));
    let lenses = lenses.as_array().expect("lenses");
    assert_eq!(lenses.len(), 2);
    assert_eq!(lenses[1]["command"]["command"], "php.runTest");
    assert_eq!(lenses[1]["command"]["arguments"][0], listed[1]);
    assert_eq!(lenses[1]["range"], listed[1]["range"]);
    client.shutdown();
}

#[test]
fn a_provider_string_leads_to_its_method_completes_and_renames() {
    let disk = project();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/tests/Unit/FooTest.php");
    client.open(&uri, FOO_TEST);

    let definition = client.at("textDocument/definition", &uri, 8, 23);
    assert_eq!(definition[0]["uri"], uri);
    assert_eq!(definition[0]["range"]["start"]["line"], 11);

    let found = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": 11, "character": 30 },
            "context": { "includeDeclaration": true }
        }),
    );
    let lines: Vec<u64> = found
        .as_array()
        .expect("locations")
        .iter()
        .map(|location| location["range"]["start"]["line"].as_u64().unwrap())
        .collect();
    assert_eq!(lines, vec![8, 11]);

    let renamed = client.request(
        "textDocument/rename",
        json!({ "textDocument": { "uri": uri }, "position": { "line": 8, "character": 23 }, "newName": "rows" }),
    );
    let edits = renamed["changes"][&uri].as_array().expect("edits");
    assert_eq!(edits.len(), 2);
    assert!(edits.iter().all(|edit| edit["newText"] == "rows"));

    let edited = FOO_TEST.replace("#[DataProvider('numbers')]", "#[DataProvider('')]");
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 2 },
            "contentChanges": [{ "text": edited }]
        }),
    );
    let completion = client.at("textDocument/completion", &uri, 8, 20);
    let labels: Vec<&str> = completion["items"]
        .as_array()
        .or_else(|| completion.as_array())
        .expect("items")
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, vec!["numbers"]);
    client.shutdown();
}

#[test]
fn pest_closures_run_as_the_test_case_bound_to_their_folder() {
    let disk = project();
    disk.write(
        "project/tests/Support/AppCase.php",
        "<?php\nnamespace Tests\\Support;\nabstract class AppCase extends \\PHPUnit\\Framework\\TestCase {\n    public function login(): string { return ''; }\n}\n",
    );
    disk.write(
        "project/tests/Pest.php",
        "<?php\nuses(Tests\\Support\\AppCase::class)->in('Feature');\n",
    );
    disk.write(
        "project/tests/Pest/functions.php",
        "<?php\nfunction it(string $d, ?Closure $c = null) {}\nfunction beforeEach(?Closure $c = null) {}\nfunction describe(string $d, Closure $c) {}\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/tests/Feature/LoginTest.php");
    let text = "<?php\nbeforeEach(function () {\n    $this->name = 'bas';\n});\n\ndescribe('login', function () {\n    it('works', function () {\n        $this->\n    });\n});\n";
    client.open(&uri, text);

    let completion = client.at("textDocument/completion", &uri, 7, 15);
    let labels: Vec<&str> = completion["items"]
        .as_array()
        .or_else(|| completion.as_array())
        .expect("items")
        .iter()
        .map(|item| item["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"login"), "{labels:?}");
    assert!(labels.contains(&"assertTrue"), "{labels:?}");
    assert!(labels.contains(&"name"), "{labels:?}");

    let listed = client.request("php/runnables", json!({ "uri": uri }));
    let found: Vec<(&str, &str)> = listed
        .as_array()
        .expect("a list")
        .iter()
        .map(|runnable| (runnable["kind"].as_str().unwrap(), runnable["label"].as_str().unwrap()))
        .collect();
    assert_eq!(found, vec![("pest", "login"), ("pest", "login → it works")]);
    let plain = disk.uri("project/src/Models/User.php");
    client.open(&plain, "<?php\nclass User {}\n");
    let nothing: Value = client.request("php/runnables", json!({ "uri": plain }));
    assert_eq!(nothing, json!([]));
    client.shutdown();
}
