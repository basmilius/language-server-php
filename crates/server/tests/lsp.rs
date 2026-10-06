//! Talks to the server over an in-memory connection, the way an editor talks to it over stdio.

mod common;

use std::time::Duration;

use common::*;
use lsp_server::{Message, Request, RequestId, Response};
use serde_json::{Value, json};

fn messages(diagnostics: &[Value]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|found| found["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

const URI: &str = "file:///project/a.php";

#[test]
fn initializes_with_the_capabilities_it_has() {
    let (client, result) = Client::start(json!({}), Value::Null);
    let capabilities = &result["capabilities"];
    assert_eq!(capabilities["textDocumentSync"]["change"], 2);
    assert_eq!(capabilities["documentSymbolProvider"], true);
    assert_eq!(capabilities["foldingRangeProvider"], true);
    assert_eq!(capabilities["selectionRangeProvider"], true);
    assert_eq!(capabilities["positionEncoding"], "utf-16");
    assert!(
        capabilities["diagnosticProvider"].is_null(),
        "no pull diagnostics for a client that cannot pull"
    );
    assert_eq!(result["serverInfo"]["name"], "php-language-server");
    client.shutdown();
}

#[test]
fn negotiates_utf8_positions() {
    let (client, result) = Client::start(
        json!({ "general": { "positionEncodings": ["utf-16", "utf-8"] } }),
        Value::Null,
    );
    assert_eq!(result["capabilities"]["positionEncoding"], "utf-8");
    client.shutdown();
}

#[test]
fn publishes_syntax_errors_and_fixes_them_after_an_incremental_change() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, "<?php\n$a = ;\n");
    let found = client.diagnostics(URI);
    assert_eq!(messages(&found), ["Expression expected"]);
    assert_eq!(found[0]["severity"], 1);
    assert_eq!(found[0]["source"], "php");
    assert_eq!(
        found[0]["range"]["start"],
        json!({ "line": 1, "character": 3 }),
        "an empty range widens to the character before it"
    );
    client.notify(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": URI, "version": 2 },
            "contentChanges": [{ "range": { "start": { "line": 1, "character": 5 }, "end": { "line": 1, "character": 5 } }, "text": "1" }]
        }),
    );
    assert!(client.diagnostics(URI).is_empty());
    client.notify("textDocument/didClose", json!({ "textDocument": { "uri": URI } }));
    assert!(client.diagnostics(URI).is_empty(), "closing clears the diagnostics");
    client.shutdown();
}

#[test]
fn a_burst_of_changes_ends_in_the_diagnostics_of_the_last_text() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, "<?php\n$a = 1;\n");
    assert!(client.diagnostics(URI).is_empty());
    for version in 2..30 {
        let text = if version % 2 == 0 {
            "<?php\n$a = ;\n"
        } else {
            "<?php\n$a = 1;\n"
        };
        client.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": URI, "version": version }, "contentChanges": [{ "text": text }] }),
        );
    }
    let mut last = Vec::new();
    // The final version is 29, which is valid; keep reading until the server has gone quiet.
    while let Ok(Message::Notification(notification)) =
        client.connection.receiver.recv_timeout(Duration::from_millis(300))
    {
        last = notification.params["diagnostics"]
            .as_array()
            .cloned()
            .unwrap_or_default();
    }
    assert!(last.is_empty());
    client.shutdown();
}

#[test]
fn reports_syntax_newer_than_the_language_level() {
    let text = "<?php\nclass A { public int $x { get => 1; } }\n";
    let (mut client, _) = Client::start(json!({}), json!({ "phpVersion": "8.3" }));
    client.open(URI, text);
    assert_eq!(
        messages(&client.diagnostics(URI)),
        ["Property hooks are only available since PHP 8.4"]
    );
    client.shutdown();

    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, text);
    assert!(client.diagnostics(URI).is_empty(), "the newest level is the default");
    client.shutdown();
}

#[test]
fn a_pushed_setting_changes_the_level_of_open_documents() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, "<?php\n$a = $b |> strlen(...);\n");
    assert!(client.diagnostics(URI).is_empty());
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "phpLanguageServer": { "phpVersion": "8.4" } } }),
    );
    assert_eq!(
        messages(&client.diagnostics(URI)),
        ["The pipe operator is only available since PHP 8.5"]
    );
    client.shutdown();
}

#[test]
fn asks_the_client_for_the_level_of_each_document() {
    let (mut client, _) = Client::start(json!({ "workspace": { "configuration": true } }), Value::Null);
    client.open(URI, "<?php\n$a = match (1) { default => 2 };\n");
    let (id, params) = client.wait_for(|message| match message {
        Message::Request(request) if request.method == "workspace/configuration" => {
            Some((request.id.clone(), request.params.clone()))
        }
        _ => None,
    });
    assert_eq!(params["items"][0]["scopeUri"], URI);
    assert_eq!(params["items"][0]["section"], "phpLanguageServer");
    client.send(Response::new_ok(id, json!([{ "phpVersion": "7.4" }])));
    let found = messages(&client.wait_for(|message| {
        match message {
            Message::Notification(notification)
                if notification.method == "textDocument/publishDiagnostics"
                    && notification.params["diagnostics"]
                        .as_array()
                        .is_some_and(|items| !items.is_empty()) =>
            {
                Some(
                    notification.params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                )
            }
            _ => None,
        }
    }));
    assert_eq!(found, ["Match expressions are only available since PHP 8.0"]);
    client.shutdown();
}

#[test]
fn answers_with_pulled_diagnostics_for_a_client_that_pulls() {
    let (mut client, result) = Client::start(json!({ "textDocument": { "diagnostic": {} } }), Value::Null);
    assert_eq!(
        result["capabilities"]["diagnosticProvider"]["interFileDependencies"],
        false
    );
    client.open(URI, "<?php\n$a = ;\n");
    let report = client.request("textDocument/diagnostic", json!({ "textDocument": { "uri": URI } }));
    assert_eq!(report["kind"], "full");
    assert_eq!(
        messages(report["items"].as_array().expect("items")),
        ["Expression expected"]
    );
    assert!(
        !client.backlog.iter().any(
            |message| matches!(message, Message::Notification(n) if n.method == "textDocument/publishDiagnostics")
        ),
        "a client that pulls is not sent diagnostics"
    );
    client.shutdown();
}

const SOURCE: &str = "<?php\nnamespace App;\n\nuse A\\B;\nuse A\\C;\n\n/** Doc\n * more */\nfinal class User\n{\n    public const X = 1;\n    public function name(): string\n    {\n        return [\n            1,\n        ][0];\n    }\n}\n";

#[test]
fn lists_document_symbols_as_a_tree_or_flat() {
    let (mut client, _) = Client::start(
        json!({ "textDocument": { "documentSymbol": { "hierarchicalDocumentSymbolSupport": true } } }),
        Value::Null,
    );
    client.open(URI, SOURCE);
    let symbols = client.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": URI } }));
    let namespace = &symbols[0];
    assert_eq!(namespace["name"], "App");
    assert_eq!(namespace["kind"], 3);
    let class = &namespace["children"][0];
    assert_eq!(class["name"], "User");
    assert_eq!(class["kind"], 5);
    assert_eq!(class["children"][0]["name"], "X");
    assert_eq!(class["children"][1]["name"], "name");
    assert_eq!(class["children"][1]["detail"], "(): string");
    assert_eq!(class["selectionRange"]["start"], json!({ "line": 8, "character": 12 }));
    client.shutdown();

    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, SOURCE);
    let symbols = client.request("textDocument/documentSymbol", json!({ "textDocument": { "uri": URI } }));
    let names: Vec<_> = symbols
        .as_array()
        .expect("a list")
        .iter()
        .map(|symbol| {
            (
                symbol["name"].as_str().unwrap_or_default(),
                symbol["containerName"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("App", None),
            ("User", Some("App")),
            ("X", Some("User")),
            ("name", Some("User"))
        ]
    );
    client.shutdown();
}

#[test]
fn answers_folding_and_selection_ranges() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, SOURCE);
    let folds = client.request("textDocument/foldingRange", json!({ "textDocument": { "uri": URI } }));
    let folds: Vec<(i64, i64, Option<&str>)> = folds
        .as_array()
        .expect("a list")
        .iter()
        .map(|fold| {
            (
                fold["startLine"].as_i64().unwrap_or(-1),
                fold["endLine"].as_i64().unwrap_or(-1),
                fold["kind"].as_str(),
            )
        })
        .collect();
    assert!(folds.contains(&(3, 4, Some("imports"))), "{folds:?}");
    assert!(folds.contains(&(6, 7, Some("comment"))), "{folds:?}");
    assert!(folds.contains(&(9, 16, None)), "{folds:?}");
    assert!(folds.contains(&(12, 15, None)), "{folds:?}");

    let ranges = client.request(
        "textDocument/selectionRange",
        json!({ "textDocument": { "uri": URI }, "positions": [{ "line": 11, "character": 21 }] }),
    );
    let mut depth = 0;
    let mut link = &ranges[0];
    let innermost = link["range"].clone();
    while !link["parent"].is_null() {
        link = &link["parent"];
        depth += 1;
    }
    assert!(depth >= 3);
    assert_eq!(innermost["start"]["line"], 11);
    assert_eq!(link["range"]["start"], json!({ "line": 0, "character": 0 }));
    client.shutdown();
}

#[test]
fn rejects_what_it_does_not_know() {
    let (client, _) = Client::start(json!({}), Value::Null);
    client.send(Request::new(
        RequestId::from(99),
        "textDocument/linkedEditingRange".to_string(),
        json!({}),
    ));
    let response = client.connection.receiver.recv_timeout(TIMEOUT).expect("an answer");
    let Message::Response(response) = response else {
        panic!("expected a response");
    };
    assert_eq!(response.response_result.expect_err("unsupported").code, -32601);
    client.shutdown();
}

// An index over a real folder ------------------------------------------------------------------

#[test]
fn indexes_a_project_and_reports_progress() {
    let disk = Disk::new();
    let client = indexed_server(&disk);
    assert!(
        std::fs::read_dir(disk.path("storage/cache"))
            .expect("a cache folder")
            .count()
            >= 2,
        "the project and the stubs are cached"
    );
    client.shutdown();
}

#[test]
fn a_second_run_reads_the_cache() {
    let disk = Disk::new();
    indexed_server(&disk).shutdown();
    let mut client = indexed_server(&disk);
    let log = client.wait_for(|message| match message {
        Message::Notification(notification)
            if notification.method == "window/logMessage"
                && notification.params["message"]
                    .as_str()
                    .is_some_and(|text| text.starts_with("Indexed ")) =>
        {
            notification.params["message"].as_str().map(str::to_string)
        }
        _ => None,
    });
    assert!(log.contains("0 parsed"), "{log}");
    client.shutdown();
}

#[test]
fn completes_a_vendor_class_with_an_import_and_hides_newer_stubs() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    let text =
        "<?php\nnamespace App;\n\nclass Page\n{\n    public function show()\n    {\n        $w = new Wid\n    }\n}\n";
    client.open(&uri, text);
    let result = client.at("textDocument/completion", &uri, 7, 20);
    let items = result["items"].as_array().expect("items");
    let widget = items.iter().find(|item| item["label"] == "Widget").expect("Widget");
    assert_eq!(widget["kind"], 7);
    assert_eq!(widget["labelDetails"]["description"], "Acme\\Lib");
    assert_eq!(widget["textEdit"]["newText"], "Widget");
    assert_eq!(
        widget["additionalTextEdits"][0]["newText"],
        "\nuse Acme\\Lib\\Widget;\n"
    );
    assert_eq!(
        widget["additionalTextEdits"][0]["range"]["start"],
        json!({ "line": 2, "character": 0 })
    );

    let resolved = client.request("completionItem/resolve", widget.clone());
    assert_eq!(resolved["documentation"]["kind"], "markdown");

    let text = "<?php\nnamespace App;\nfunction f() {\n    array_\n}\n";
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let result = client.at("textDocument/completion", &uri, 3, 10);
    let labels: Vec<&str> = result["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(
        !labels.contains(&"array_find"),
        "an 8.1 project is not offered 8.4 functions: {labels:?}"
    );

    let text = "<?php\nnamespace App;\nfunction f() {\n    strl\n}\n";
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 3 }, "contentChanges": [{ "text": text }] }),
    );
    let result = client.at("textDocument/completion", &uri, 3, 8);
    let strlen = result["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|item| item["label"] == "strlen")
        .cloned()
        .expect("strlen");
    assert_eq!(strlen["detail"], "(string $string): int");
    assert!(
        strlen["additionalTextEdits"].is_null(),
        "global functions need no import"
    );
    client.shutdown();
}

#[test]
fn stubs_follow_the_extensions_the_project_requires() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, "<?php\nnew Redi\n");
    let result = client.at("textDocument/completion", &uri, 1, 8);
    let labels: Vec<&str> = result["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"Redis"), "ext-redis is required: {labels:?}");
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri, "version": 2 }, "contentChanges": [{ "text": "<?php\nnew Swoo\n" }] }),
    );
    let result = client.at("textDocument/completion", &uri, 1, 8);
    assert!(
        result["items"].as_array().expect("items").is_empty(),
        "ext-swoole is not"
    );
    client.shutdown();
}

#[test]
fn completes_members_of_an_inferred_type() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Models\\User;\nfunction f() {\n    $user = User::find(1);\n    $user->\n}\n",
    );
    let result = client.at("textDocument/completion", &uri, 4, 11);
    let items = result["items"].as_array().expect("items");
    let posts = items.iter().find(|item| item["label"] == "posts").expect("posts");
    assert_eq!(posts["kind"], 2);
    assert_eq!(posts["detail"], "(): array");
    assert!(items.iter().any(|item| item["label"] == "name" && item["kind"] == 10));
    client.shutdown();
}

#[test]
fn hovers_and_navigates_across_files() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Models\\User;\nfunction f() {\n    $user = User::find(1);\n    $user->posts();\n}\n",
    );
    let hover = client.at("textDocument/hover", &uri, 3, 20);
    let text = hover["contents"]["value"].as_str().expect("markdown");
    assert!(
        text.starts_with(
            "**App\\Models\\User::find**\n\n```php\npublic static function find(int $id): ?static\n```\n\nFinds a user."
        ),
        "{text}"
    );
    assert!(!text.contains("Defined in"), "{text}");
    assert_eq!(hover["range"]["start"]["line"], 3);

    let definition = client.at("textDocument/definition", &uri, 3, 20);
    assert_eq!(definition[0]["uri"], disk.uri("project/src/Models/User.php"));
    assert_eq!(definition[0]["range"]["start"], json!({ "line": 9, "character": 27 }));

    let type_definition = client.at("textDocument/typeDefinition", &uri, 4, 6);
    assert_eq!(type_definition[0]["uri"], disk.uri("project/src/Models/User.php"));

    let class_definition = client.at("textDocument/definition", &uri, 3, 14);
    assert_eq!(
        class_definition[0]["range"]["start"],
        json!({ "line": 4, "character": 6 })
    );
    client.shutdown();
}

#[test]
fn finds_implementations_and_workspace_symbols() {
    let disk = Disk::new();
    disk.write(
        "project/src/Shapes.php",
        "<?php\nnamespace App;\ninterface Shape { public function area(): float; }\nclass Circle implements Shape { public function area(): float { return 1.0; } }\nclass Square implements Shape { public function area(): float { return 2.0; } }\n",
    );
    let mut client = indexed_server(&disk);
    let shapes = disk.uri("project/src/Shapes.php");
    client.open(
        &shapes,
        &std::fs::read_to_string(disk.path("project/src/Shapes.php")).expect("read"),
    );
    let implementations = client.at("textDocument/implementation", &shapes, 2, 12);
    assert_eq!(implementations.as_array().map(Vec::len), Some(2));
    let methods = client.at("textDocument/implementation", &shapes, 2, 36);
    assert_eq!(methods.as_array().map(Vec::len), Some(2));

    let symbols = client.request("workspace/symbol", json!({ "query": "Circ" }));
    assert_eq!(symbols[0]["name"], "Circle");
    assert_eq!(symbols[0]["containerName"], "App");
    assert_eq!(symbols[0]["location"]["uri"], shapes);
    let widget = client.request("workspace/symbol", json!({ "query": "Widget" }));
    assert_eq!(
        widget[0]["location"]["uri"],
        disk.uri("project/vendor/acme/lib/src/Widget.php")
    );
    client.shutdown();
}

#[test]
fn the_language_level_follows_composer() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Hooks.php");
    client.open(&uri, "<?php\nclass A { public int $x { get => 1; } }\n");
    assert_eq!(
        messages(&client.diagnostics(&uri)),
        ["Property hooks are only available since PHP 8.4"],
        "composer.json says 8.1 and the server's default is the newest"
    );
    client.shutdown();
}

#[test]
fn watched_files_update_the_index() {
    let disk = Disk::new();
    let (mut client, _) = Client::start_in(
        json!({ "window": { "workDoneProgress": true }, "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }),
        disk.options(),
        json!(disk.uri("project")),
    );
    let registration = client.wait_for(|message| match message {
        Message::Request(request) if request.method == "client/registerCapability" => Some(request.clone()),
        _ => None,
    });
    assert_eq!(
        registration.params["registrations"][0]["method"],
        "workspace/didChangeWatchedFiles"
    );
    client.send(Response::new_ok(registration.id, Value::Null));
    client.wait_for_indexing();

    disk.write("project/src/Fresh.php", "<?php\nnamespace App;\nclass Freshly {}\n");
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": disk.uri("project/src/Fresh.php"), "type": 1 }] }),
    );
    let found = client.request("workspace/symbol", json!({ "query": "Freshly" }));
    assert_eq!(found[0]["name"], "Freshly");

    std::fs::remove_file(disk.path("project/src/Fresh.php")).expect("removed");
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": disk.uri("project/src/Fresh.php"), "type": 3 }] }),
    );
    let gone = client.request("workspace/symbol", json!({ "query": "Freshly" }));
    assert_eq!(gone.as_array().map(Vec::len), Some(0));
    client.shutdown();
}

#[test]
fn open_documents_win_over_the_disk() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Models/User.php");
    client.open(
        &uri,
        "<?php\nnamespace App\\Models;\nclass User { public function brandNew(): int {} }\n",
    );
    let page = disk.uri("project/src/Page.php");
    client.open(
        &page,
        "<?php\nuse App\\Models\\User;\nfunction f(User $u) {\n    $u->\n}\n",
    );
    let result = client.at("textDocument/completion", &page, 3, 8);
    let labels: Vec<&str> = result["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"brandNew") && !labels.contains(&"posts"), "{labels:?}");
    client.shutdown();
}

#[test]
fn reads_a_class_that_composer_points_at_but_the_index_skipped() {
    let disk = Disk::new();
    disk.write(
        "project/composer.json",
        r#"{"autoload":{"psr-4":{"App\\":"src/","Hidden\\":".hidden/src/"}}}"#,
    );
    disk.write(
        "project/.hidden/src/Thing.php",
        "<?php\nnamespace Hidden;\n/** Found through the autoload map. */\nclass Thing {}\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, "<?php\nnew \\Hidden\\Thing();\n");
    let hover = client.at("textDocument/hover", &uri, 1, 12);
    let text = hover["contents"]["value"].as_str().expect("markdown");
    assert!(text.contains("Found through the autoload map."), "{text}");
    let definition = client.at("textDocument/definition", &uri, 1, 12);
    assert_eq!(definition[0]["uri"], disk.uri("project/.hidden/src/Thing.php"));
    client.shutdown();
}

#[test]
fn reads_the_classes_of_composer_s_generated_class_map() {
    let disk = Disk::new();
    disk.write(
        "project/composer.json",
        r#"{"autoload":{"classmap":["lib/"]},"config":{"classmap-authoritative":true}}"#,
    );
    disk.write(
        "project/lib/legacy.inc",
        "<?php\n/** Found through the class map. */\nclass LegacyThing {}\n",
    );
    disk.write(
        "project/vendor/composer/autoload_classmap.php",
        "<?php\n\n$vendorDir = dirname(__DIR__);\n$baseDir = dirname($vendorDir);\n\nreturn array(\n    'LegacyThing' => $baseDir . '/lib/legacy.inc',\n);\n",
    );
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, "<?php\nnew \\LegacyThing();\n");
    let definition = client.at("textDocument/definition", &uri, 1, 8);
    assert_eq!(definition[0]["uri"], disk.uri("project/lib/legacy.inc"), "{definition}");
    let diagnostics = client.diagnostics(&uri);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic["code"] != "undefined-class"),
        "{diagnostics:?}"
    );
    client.shutdown();
}

#[test]
fn a_type_alias_types_what_its_docs_name_and_leads_to_its_declaration() {
    let disk = Disk::new();
    disk.write("project/composer.json", r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#);
    disk.write(
        "project/src/Shapes.php",
        "<?php\nnamespace App;\n\n/**\n * @psalm-type Point = array{x: int, y: int}\n */\nclass Shapes\n{\n}\n",
    );
    let canvas = "<?php\nnamespace App;\n\n/** @phpstan-import-type Point from Shapes */\nclass Canvas\n{\n    /** @return Point */\n    public function origin() {}\n}\n";
    disk.write("project/src/Canvas.php", canvas);
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Canvas.php");
    client.open(&uri, canvas);
    let definition = client.at("textDocument/definition", &uri, 6, 18);
    assert_eq!(definition[0]["uri"], disk.uri("project/src/Shapes.php"), "{definition}");
    assert_eq!(definition[0]["range"]["start"]["line"], 4, "{definition}");
    let page = disk.uri("project/src/page.php");
    client.open(&page, "<?php\nnamespace App;\nfunction page(Canvas $canvas) {\n    $point = $canvas->origin();\n    return $point;\n}\n");
    let hover = client.at("textDocument/hover", &page, 4, 13);
    let text = hover["contents"]["value"].as_str().expect("markdown");
    assert!(text.contains("array{x: int, y: int}"), "{text}");
    client.shutdown();
}

#[test]
fn finds_usages_and_highlights_across_the_project() {
    let disk = Disk::new();
    disk.write(
        "project/src/Controller.php",
        "<?php\nnamespace App;\n\nuse App\\Models\\User;\n\nclass Controller\n{\n    public function show(int $id): ?User\n    {\n        $user = User::find($id);\n        return $user;\n    }\n}\n",
    );
    let mut client = indexed_server(&disk);
    let page = disk.uri("project/src/Page.php");
    client.open(
        &page,
        "<?php\nuse App\\Models\\User;\nfunction f(User $u) { return $u->name; }\n",
    );
    let found = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": page },
            "position": { "line": 2, "character": 12 },
            "context": { "includeDeclaration": true }
        }),
    );
    let mut places: Vec<String> = found
        .as_array()
        .expect("locations")
        .iter()
        .map(|location| {
            format!(
                "{}:{}",
                location["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default(),
                location["range"]["start"]["line"]
            )
        })
        .collect();
    places.sort();
    assert_eq!(
        places,
        [
            "Controller.php:3",
            "Controller.php:7",
            "Controller.php:9",
            "Page.php:1",
            "Page.php:2",
            "User.php:4"
        ]
    );
    let without = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": page },
            "position": { "line": 2, "character": 12 },
            "context": { "includeDeclaration": false }
        }),
    );
    assert_eq!(without.as_array().map(Vec::len), Some(5));

    let highlights = client.at("textDocument/documentHighlight", &page, 2, 12);
    let kinds: Vec<(i64, i64)> = highlights
        .as_array()
        .expect("highlights")
        .iter()
        .map(|item| {
            (
                item["range"]["start"]["line"].as_i64().unwrap_or(-1),
                item["kind"].as_i64().unwrap_or(-1),
            )
        })
        .collect();
    assert_eq!(kinds, [(1, 1), (2, 1)]);
    let variable = client.at("textDocument/documentHighlight", &page, 2, 14);
    assert_eq!(variable.as_array().map(Vec::len), Some(2));
    client.shutdown();
}

#[test]
fn renames_a_class_and_moves_its_file() {
    let disk = Disk::new();
    let mut client = {
        let capabilities = json!({
            "window": { "workDoneProgress": true },
            "workspace": { "workspaceEdit": { "documentChanges": true, "resourceOperations": ["rename"] } }
        });
        let (mut client, _) = Client::start_in(capabilities, disk.options(), json!(disk.uri("project")));
        client.wait_for_indexing();
        client
    };
    let page = disk.uri("project/src/Page.php");
    client.open(&page, "<?php\nuse App\\Models\\User;\nfunction f(User $u) {}\n");
    let prepared = client.at("textDocument/prepareRename", &page, 2, 12);
    assert_eq!(prepared["placeholder"], "User");
    let edit = client.request(
        "textDocument/rename",
        json!({ "textDocument": { "uri": page }, "position": { "line": 2, "character": 12 }, "newName": "Member" }),
    );
    let changes = edit["documentChanges"].as_array().expect("document changes");
    let rename = changes
        .iter()
        .find(|change| change["kind"] == "rename")
        .expect("a file rename");
    assert_eq!(rename["oldUri"], disk.uri("project/src/Models/User.php"));
    assert_eq!(rename["newUri"], disk.uri("project/src/Models/Member.php"));
    let page_edit = changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == page)
        .expect("edits of the open page");
    assert_eq!(page_edit["textDocument"]["version"], 1);
    assert_eq!(page_edit["edits"].as_array().map(Vec::len), Some(2));
    assert_eq!(page_edit["edits"][0]["newText"], "Member");
    let user_edit = changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == disk.uri("project/src/Models/User.php"))
        .expect("edits of the declaring file");
    assert!(user_edit["textDocument"]["version"].is_null());
    assert_eq!(rename_order(changes), "edits first");

    let refused = client.request_error(
        "textDocument/rename",
        json!({ "textDocument": { "uri": page }, "position": { "line": 2, "character": 12 }, "newName": "class" }),
    );
    assert_eq!(refused, "'class' is a reserved word");
    client.shutdown();
}

fn rename_order(changes: &[Value]) -> &'static str {
    let last_edit = changes.iter().rposition(|change| change.get("textDocument").is_some());
    let rename = changes.iter().position(|change| change["kind"] == "rename");
    match (last_edit, rename) {
        (Some(edit), Some(rename)) if edit < rename => "edits first",
        _ => "wrong order",
    }
}

#[test]
fn a_client_without_documentchanges_gets_plain_changes() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let page = disk.uri("project/src/Page.php");
    client.open(
        &page,
        "<?php\nuse App\\Models\\User;\nfunction f(User $u) { return $u->name; }\n",
    );
    let edit = client.request(
        "textDocument/rename",
        json!({ "textDocument": { "uri": page }, "position": { "line": 2, "character": 34 }, "newName": "length" }),
    );
    assert!(edit["documentChanges"].is_null());
    let changes = edit["changes"].as_object().expect("changes");
    assert_eq!(changes.len(), 2);
    assert_eq!(
        changes[&disk.uri("project/src/Models/User.php")][0]["newText"],
        "$length"
    );
    assert_eq!(changes[&page][0]["newText"], "length");
    client.shutdown();
}

#[test]
fn offers_signature_help_for_the_call_under_the_cursor() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Models\\User;\nfunction f() {\n    User::find(1);\n    strlen('');\n}\n",
    );
    let help = client.at("textDocument/signatureHelp", &uri, 3, 15);
    assert_eq!(help["signatures"][0]["label"], "find(int $id): ?static");
    assert_eq!(help["signatures"][0]["parameters"][0]["label"], "int $id");
    assert_eq!(help["activeParameter"], 0);
    assert_eq!(help["signatures"][0]["documentation"]["value"], "Finds a user.");
    let outside = client.at("textDocument/signatureHelp", &uri, 2, 5);
    assert!(outside.is_null());
    let builtin = client.at("textDocument/signatureHelp", &uri, 4, 11);
    assert_eq!(builtin["signatures"][0]["label"], "strlen(string $string): int");
    client.shutdown();
}

#[test]
fn walks_call_and_type_hierarchies() {
    let disk = Disk::new();
    disk.write(
        "project/src/Controller.php",
        "<?php\nnamespace App;\n\nuse App\\Models\\User;\n\nclass Controller\n{\n    public function show(int $id): ?User\n    {\n        return User::find($id);\n    }\n}\n",
    );
    disk.write(
        "project/src/Admin.php",
        "<?php\nnamespace App;\n\nuse App\\Models\\User;\n\nclass Admin extends User {}\n",
    );
    let mut client = indexed_server(&disk);
    let user = disk.uri("project/src/Models/User.php");
    client.open(
        &user,
        &std::fs::read_to_string(disk.path("project/src/Models/User.php")).expect("read"),
    );

    let prepared = client.at("textDocument/prepareCallHierarchy", &user, 9, 30);
    let item = prepared[0].clone();
    assert_eq!(item["name"], "find");
    assert_eq!(item["detail"], "App\\Models\\User");
    let incoming = client.request("callHierarchy/incomingCalls", json!({ "item": item }));
    assert_eq!(incoming[0]["from"]["name"], "show");
    assert_eq!(incoming[0]["from"]["detail"], "App\\Controller");
    assert_eq!(
        incoming[0]["fromRanges"][0]["start"],
        json!({ "line": 9, "character": 21 })
    );

    let show = client.request("callHierarchy/outgoingCalls", json!({ "item": incoming[0]["from"] }));
    assert_eq!(show[0]["to"]["name"], "find");
    assert_eq!(show[0]["to"]["uri"], user);

    let types = client.at("textDocument/prepareTypeHierarchy", &user, 4, 8);
    assert_eq!(types[0]["name"], "User");
    let subtypes = client.request("typeHierarchy/subtypes", json!({ "item": types[0] }));
    assert_eq!(subtypes[0]["name"], "Admin");
    let supertypes = client.request("typeHierarchy/supertypes", json!({ "item": subtypes[0] }));
    assert_eq!(supertypes[0]["name"], "User");
    client.shutdown();
}

#[test]
fn serves_semantic_tokens_in_the_delta_encoding_of_its_legend() {
    let disk = Disk::new();
    let (mut client, result) = Client::start_in(PROGRESS_CAPABILITIES(), disk.options(), json!(disk.uri("project")));
    client.wait_for_indexing();
    let legend = &result["capabilities"]["semanticTokensProvider"]["legend"];
    let types: Vec<&str> = legend["tokenTypes"]
        .as_array()
        .expect("types")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let modifiers: Vec<&str> = legend["tokenModifiers"]
        .as_array()
        .expect("modifiers")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(result["capabilities"]["semanticTokensProvider"]["range"], true);
    assert_eq!(result["capabilities"]["semanticTokensProvider"]["full"], true);
    assert!(types.contains(&"class") && types.contains(&"method") && types.contains(&"parameter"));
    assert!(modifiers.contains(&"static") && modifiers.contains(&"defaultLibrary"));

    let uri = disk.uri("project/src/Page.php");
    client.open(
        &uri,
        "<?php\nuse App\\Models\\User;\nfunction f(User $u) {\n    return User::find(1);\n}\n",
    );
    let tokens = client.request(
        "textDocument/semanticTokens/full",
        json!({ "textDocument": { "uri": uri } }),
    );
    let data: Vec<u64> = tokens["data"]
        .as_array()
        .expect("data")
        .iter()
        .filter_map(Value::as_u64)
        .collect();
    let mut decoded = Vec::new();
    let (mut line, mut start) = (0, 0);
    for token in data.chunks(5) {
        line += token[0];
        start = if token[0] == 0 { start + token[1] } else { token[1] };
        let mods: Vec<&str> = modifiers
            .iter()
            .enumerate()
            .filter(|(bit, _)| token[4] & (1 << bit) != 0)
            .map(|(_, name)| *name)
            .collect();
        decoded.push(format!(
            "{line}:{start}+{} {} {}",
            token[2],
            types[token[3] as usize],
            mods.join(",")
        ));
    }
    assert_eq!(
        decoded,
        [
            "1:4+10 namespace ",
            "1:15+4 class ",
            "2:9+1 function declaration",
            "2:11+4 class ",
            "2:16+2 parameter declaration",
            "3:11+4 class ",
            "3:17+4 method static",
        ]
    );
    let ranged = client.request(
        "textDocument/semanticTokens/range",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 4, "character": 0 } }
        }),
    );
    assert_eq!(ranged["data"].as_array().map(Vec::len), Some(10));
    client.shutdown();
}

#[test]
fn serves_inlay_hints_and_follows_the_settings() {
    let disk = Disk::new();
    let (mut client, result) = Client::start_in(
        json!({ "window": { "workDoneProgress": true }, "workspace": { "inlayHint": { "refreshSupport": true } } }),
        disk.options(),
        json!(disk.uri("project")),
    );
    assert_eq!(result["capabilities"]["inlayHintProvider"], true);
    client.wait_for_indexing();
    let uri = disk.uri("project/src/Page.php");
    client.open(
        &uri,
        "<?php\nfunction pad(string $text, int $width) {}\npad('a', 3);\narray_map(fn($x) => $x, []);\n",
    );
    let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 4, "character": 0 } });
    let hints = client.request(
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri }, "range": range }),
    );
    let labels: Vec<(&str, i64)> = hints
        .as_array()
        .expect("hints")
        .iter()
        .map(|hint| {
            (
                hint["label"].as_str().unwrap_or_default(),
                hint["kind"].as_i64().unwrap_or(0),
            )
        })
        .collect();
    assert_eq!(labels, [("text:", 2), ("width:", 2)]);
    assert_eq!(hints[0]["position"], json!({ "line": 2, "character": 4 }));
    assert_eq!(hints[0]["paddingRight"], true);
    assert_eq!(hints[0]["textEdits"][0]["newText"], "text: ");

    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "phpLanguageServer": { "inlayHints": { "parameterNames": false } } } }),
    );
    client.wait_for(|message| match message {
        Message::Request(request) if request.method == "workspace/inlayHint/refresh" => Some(()),
        _ => None,
    });
    let hints = client.request(
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri }, "range": range }),
    );
    assert_eq!(hints.as_array().map(Vec::len), Some(0));
    client.shutdown();
}

#[test]
fn completes_the_methods_a_class_can_override() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Admin.php");
    client.open(
        &uri,
        "<?php\nnamespace App;\n\nuse App\\Models\\User;\n\nclass Admin extends User\n{\n    public function po\n}\n",
    );
    let result = client.at("textDocument/completion", &uri, 7, 22);
    let items = result["items"].as_array().expect("items");
    let posts = items.iter().find(|item| item["label"] == "posts").expect("posts");
    assert_eq!(posts["kind"], 2);
    assert_eq!(
        posts["textEdit"]["newText"],
        "posts(): array\n    {\n        return parent::posts();\n    }"
    );
    assert_eq!(
        posts["textEdit"]["range"]["start"],
        json!({ "line": 7, "character": 20 })
    );
    client.shutdown();
}

// Inspections ----------------------------------------------------------------------------------

fn codes(diagnostics: &[Value]) -> Vec<String> {
    diagnostics
        .iter()
        .map(|found| found["code"].as_str().unwrap_or_default().to_string())
        .collect()
}

const PAGE: &str = "<?php\nnamespace App;\n\nuse Acme\\Lib\\Widget;\nuse App\\Models\\User;\n\nclass Page\n{\n    public function show(User $user): void\n    {\n        echo strlen($user->name);\n        missing_function();\n        $unused = new Redis();\n        new SwooleServer();\n        $user->nothing();\n    }\n}\n";

#[test]
fn reports_inspections_once_the_project_is_indexed() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    let found = client.diagnostics(&uri);
    assert_eq!(
        codes(&found),
        [
            "unused-import",
            "undefined-function",
            "unused-variable",
            "undefined-class",
            "undefined-class",
            "undefined-method"
        ],
        "{found:?}"
    );
    assert_eq!(found[0]["tags"], json!([1]), "an unused import is unnecessary");
    assert_eq!(found[1]["severity"], 1);
    assert_eq!(
        found[3]["message"], "Undefined class 'Redis'",
        "a global class needs a backslash in a namespace"
    );
    client.shutdown();
}

#[test]
fn a_class_only_an_unrequired_extension_has_is_not_undefined() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, "<?php\nnamespace App;\n\nfinal class Page\n{\n    public function show(): void\n    {\n        new \\SwooleServer();\n        new \\Redis();\n        new \\Gone();\n    }\n}\n");
    let found = client.diagnostics(&uri);
    assert_eq!(messages(&found), ["Undefined class 'Gone'"]);
    client.shutdown();
}

#[test]
fn nothing_is_undefined_before_the_index_is_read() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, PAGE);
    let found = client.diagnostics(URI);
    assert!(
        codes(&found)
            .iter()
            .all(|code| code != "undefined-class" && code != "undefined-function"),
        "{found:?}"
    );
    client.shutdown();
}

#[test]
fn inspection_settings_switch_and_move_severities() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    assert_eq!(client.diagnostics(&uri).len(), 6);
    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "phpLanguageServer": { "inspections": {
            "unused-import": "off",
            "undefined-function": { "severity": "hint" },
            "unused-variable": false
        } } } }),
    );
    let found = client.diagnostics(&uri);
    assert_eq!(
        codes(&found),
        [
            "undefined-function",
            "undefined-class",
            "undefined-class",
            "undefined-method"
        ]
    );
    assert_eq!(found[0]["severity"], 4);
    client.shutdown();
}

// Code actions ---------------------------------------------------------------------------------

fn action_titles(actions: &Value) -> Vec<String> {
    actions
        .as_array()
        .expect("a list of actions")
        .iter()
        .map(|action| action["title"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn range_at(line: u32, from: u32, to: u32) -> Value {
    json!({ "start": { "line": line, "character": from }, "end": { "line": line, "character": to } })
}

#[test]
fn offers_quick_fixes_with_their_edits() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    let found = client.diagnostics(&uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(3, 0, 20), "context": { "diagnostics": found } }),
    );
    assert_eq!(action_titles(&actions), ["Remove unused import", "Organize imports"]);
    let remove = &actions[0];
    assert_eq!(remove["kind"], "quickfix");
    assert_eq!(remove["isPreferred"], true);
    assert_eq!(remove["diagnostics"][0]["code"], "unused-import");
    let edits = &remove["edit"]["changes"][&uri];
    assert_eq!(edits[0]["newText"], "");
    assert_eq!(edits[0]["range"]["start"], json!({ "line": 3, "character": 0 }));
    assert_eq!(edits[0]["range"]["end"], json!({ "line": 4, "character": 0 }));
    client.shutdown();
}

#[test]
fn only_the_kinds_asked_for_are_offered() {
    let disk = Disk::new();
    let mut client = indexed_server(&disk);
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    client.diagnostics(&uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(3, 0, 0), "context": { "diagnostics": [], "only": ["source.organizeImports"] } }),
    );
    assert_eq!(action_titles(&actions), ["Organize imports"]);
    let organized = &actions[0]["edit"]["changes"][&uri][0];
    assert_eq!(organized["range"]["start"], json!({ "line": 3, "character": 0 }));
    assert_eq!(organized["newText"], "use App\\Models\\User;\n");
    client.shutdown();
}

#[test]
fn a_client_that_resolves_gets_the_edit_of_an_expensive_action_late() {
    let disk = Disk::new();
    let capabilities = json!({
        "window": { "workDoneProgress": true },
        "textDocument": { "codeAction": { "resolveSupport": { "properties": ["edit"] } } }
    });
    let (mut client, _) = Client::start_in(capabilities, disk.options(), json!(disk.uri("project")));
    client.wait_for_indexing();
    let uri = disk.uri("project/src/Page.php");
    client.open(
        &uri,
        "<?php\nnamespace App;\n\nclass Page\n{\n    public function show(): void\n    {\n        new Widget();\n    }\n}\n",
    );
    client.diagnostics(&uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(7, 14, 14), "context": { "diagnostics": [] } }),
    );
    let import = actions
        .as_array()
        .expect("actions")
        .iter()
        .find(|action| action["title"] == "Import 'Acme\\Lib\\Widget'")
        .expect("an import")
        .clone();
    assert!(import["edit"].is_null(), "the edit waits for the resolve");
    let resolved = client.request("codeAction/resolve", import);
    let edit = &resolved["edit"]["changes"][&uri][0];
    assert_eq!(edit["newText"], "\nuse Acme\\Lib\\Widget;\n");
    client.shutdown();
}

#[test]
fn writes_the_doc_block_after_an_opened_comment_and_enter() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    let text = "<?php\nclass A\n{\n    /**\n    \n    public function run(int $a, string $b): bool\n    {\n        return true;\n    }\n}\n";
    client.open(URI, text);
    client.diagnostics(URI);
    let edits = client.request(
        "textDocument/onTypeFormatting",
        json!({
            "textDocument": { "uri": URI },
            "position": { "line": 4, "character": 4 },
            "ch": "\n",
            "options": { "tabSize": 4, "insertSpaces": true }
        }),
    );
    assert_eq!(edits[0]["range"]["start"], json!({ "line": 4, "character": 0 }));
    assert_eq!(
        edits[0]["newText"],
        "     * @param int $a\n     * @param string $b\n     * @return bool\n     */\n"
    );
    client.shutdown();
}

// Composer projects below the workspace folder -------------------------------------------------

const NESTED_PAGE: &str =
    "<?php\nnamespace App;\n\nuse Acme\\Lib\\Widget;\n\nfunction page() {\n    return new Widget();\n}\n";

/// A workspace folder with no `composer.json` of its own and two composer projects below it that
/// install the same package, plus a backup folder and loose PHP that belong to no project.
fn nested_disk() -> Disk {
    let disk = Disk {
        dir: tempfile::tempdir().expect("a temp dir"),
    };
    for (name, level, widget) in [("backend", "8.1", "Backend"), ("shop", "8.4", "Shop")] {
        disk.write(
            &format!("work/{name}/composer.json"),
            &format!(r#"{{"config":{{"platform":{{"php":"{level}"}}}},"autoload":{{"psr-4":{{"App\\":"src/"}}}}}}"#),
        );
        disk.write(&format!("work/{name}/src/Page.php"), NESTED_PAGE);
        disk.write(
            &format!("work/{name}/vendor/composer/installed.json"),
            r#"{"packages":[{"name":"acme/lib","install-path":"../acme/lib","autoload":{"psr-4":{"Acme\\Lib\\":"src/"}}}]}"#,
        );
        disk.write(
            &format!("work/{name}/vendor/acme/lib/src/Widget.php"),
            &format!("<?php\nnamespace Acme\\Lib;\n\nclass Widget\n{{\n    public function render{widget}(): string {{}}\n}}\n"),
        );
    }
    disk.write("work/tools/Helper.php", "<?php\nclass LooseHelper {}\n");
    disk.write("work/~backup/old/composer.json", "{}");
    disk.write("work/~backup/old/src/Old.php", "<?php\nclass OldBackup {}\n");
    disk.write(
        "stubs/standard/basic.php",
        "<?php\nfunction strlen(string $string): int {}\n",
    );
    disk
}

fn nested_server(disk: &Disk) -> Client {
    let (mut client, _) = Client::start_in(PROGRESS_CAPABILITIES(), disk.options(), json!(disk.uri("work")));
    client.wait_for_indexing();
    client
}

fn symbol_uris(client: &mut Client, query: &str) -> Vec<String> {
    client
        .request("workspace/symbol", json!({ "query": query }))
        .as_array()
        .expect("symbols")
        .iter()
        .map(|symbol| symbol["location"]["uri"].as_str().expect("uri").to_string())
        .collect()
}

#[test]
fn a_file_sees_the_packages_of_its_nearest_composer_root() {
    let disk = nested_disk();
    let mut client = nested_server(&disk);
    for name in ["backend", "shop"] {
        let uri = disk.uri(&format!("work/{name}/src/Page.php"));
        client.open(&uri, NESTED_PAGE);
        let definition = client.at("textDocument/definition", &uri, 6, 17);
        assert_eq!(
            definition[0]["uri"],
            disk.uri(&format!("work/{name}/vendor/acme/lib/src/Widget.php")),
            "{name} resolves to its own copy of the package"
        );
        let hover = client.at("textDocument/hover", &uri, 6, 17);
        let text = hover["contents"]["value"].as_str().expect("markdown");
        assert!(text.starts_with("**Acme\\Lib\\Widget**"), "{text}");
        assert!(!text.contains("Defined in"), "{text}");
    }
    client.shutdown();
}

#[test]
fn each_composer_root_decides_the_level_of_its_files() {
    let disk = nested_disk();
    let mut client = nested_server(&disk);
    let hooks = "<?php\nclass A { public int $x { get => 1; } }\n";
    let backend = disk.uri("work/backend/src/Hooks.php");
    client.open(&backend, hooks);
    assert_eq!(
        messages(&client.diagnostics(&backend)),
        ["Property hooks are only available since PHP 8.4"]
    );
    let shop = disk.uri("work/shop/src/Hooks.php");
    client.open(&shop, hooks);
    assert!(messages(&client.diagnostics(&shop)).is_empty());
    client.shutdown();
}

#[test]
fn a_package_in_two_roots_is_listed_once() {
    let disk = nested_disk();
    let mut client = nested_server(&disk);
    let widgets = symbol_uris(&mut client, "Widget");
    assert_eq!(widgets.len(), 1, "{widgets:?}");
    assert!(widgets[0].contains("/vendor/acme/lib/src/Widget.php"));

    let uri = disk.uri("work/shop/src/Page.php");
    let text = "<?php\nnamespace App;\nfunction f() {\n    new Wid\n}\n";
    client.open(&uri, text);
    let result = client.at("textDocument/completion", &uri, 3, 11);
    let count = result["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|item| item["label"] == "Widget")
        .count();
    assert_eq!(count, 1);

    assert_eq!(
        symbol_uris(&mut client, "strlen").len(),
        1,
        "the standard library is listed once"
    );
    client.shutdown();
}

#[test]
fn project_files_stay_with_the_project_that_holds_them() {
    let disk = nested_disk();
    let mut client = nested_server(&disk);
    assert_eq!(
        symbol_uris(&mut client, "page").len(),
        2,
        "a function of the project's own is listed for every project that has one"
    );
    let loose = symbol_uris(&mut client, "LooseHelper");
    assert_eq!(loose, [disk.uri("work/tools/Helper.php")]);
    assert_eq!(
        symbol_uris(&mut client, "OldBackup").len(),
        1,
        "a backup folder is read as part of the folder, without its own packages"
    );
    client.shutdown();
}

#[test]
fn a_folder_with_its_own_composer_json_stays_one_project() {
    let disk = nested_disk();
    disk.write("work/composer.json", r#"{"autoload":{"psr-4":{"Top\\":"top/"}}}"#);
    disk.write("work/backend/src/Deep.php", "<?php\nclass DeepInBackend {}\n");
    let mut client = nested_server(&disk);
    let uri = disk.uri("work/backend/src/Page.php");
    client.open(&uri, NESTED_PAGE);
    let hover = client.at("textDocument/hover", &uri, 6, 17);
    assert!(
        hover.is_null(),
        "backend/vendor is not read for a folder that has a composer.json: {hover}"
    );
    assert_eq!(symbol_uris(&mut client, "DeepInBackend").len(), 1);
    client.shutdown();
}

#[test]
fn a_composer_json_that_appears_below_the_folder_becomes_a_project() {
    let disk = nested_disk();
    let (mut client, _) = Client::start_in(
        json!({ "window": { "workDoneProgress": true }, "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }),
        disk.options(),
        json!(disk.uri("work")),
    );
    let registration = client.wait_for(|message| match message {
        Message::Request(request) if request.method == "client/registerCapability" => Some(request.clone()),
        _ => None,
    });
    client.send(Response::new_ok(registration.id, Value::Null));
    client.wait_for_indexing();
    assert_eq!(symbol_uris(&mut client, "Gadget").len(), 0);

    disk.write(
        "work/tools/composer.json",
        r#"{"config":{"platform":{"php":"8.2"}},"autoload":{"psr-4":{"Tools\\":"src/"}}}"#,
    );
    disk.write(
        "work/tools/src/Gadget.php",
        "<?php\nnamespace Tools;\nclass Gadget {}\n",
    );
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": disk.uri("work/tools/composer.json"), "type": 1 }] }),
    );
    client.wait_for_indexing();
    assert_eq!(
        symbol_uris(&mut client, "Gadget"),
        [disk.uri("work/tools/src/Gadget.php")]
    );
    assert!(
        symbol_uris(&mut client, "LooseHelper").len() == 1,
        "files of the folder that are not in a project stay in the folder's own"
    );

    std::fs::remove_file(disk.path("work/tools/composer.json")).expect("removed");
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": disk.uri("work/tools/composer.json"), "type": 3 }] }),
    );
    client.wait_for_indexing();
    assert_eq!(
        symbol_uris(&mut client, "Gadget"),
        [disk.uri("work/tools/src/Gadget.php")],
        "the folder reads the files of a root that is gone"
    );
    client.shutdown();
}

// Formatting ------------------------------------------------------------------------------------

/// The text after applying LSP edits. The tests use ASCII, so a character is a byte.
fn apply_edits(text: &str, edits: &Value) -> String {
    let offset = |position: &Value| {
        let line = position["line"].as_u64().expect("a line") as usize;
        let character = position["character"].as_u64().expect("a character") as usize;
        let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
        start + character
    };
    let mut edits: Vec<(usize, usize, String)> = edits
        .as_array()
        .expect("edits")
        .iter()
        .map(|edit| {
            (
                offset(&edit["range"]["start"]),
                offset(&edit["range"]["end"]),
                edit["newText"].as_str().expect("text").to_string(),
            )
        })
        .collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut out = text.to_string();
    for (start, end, replacement) in edits {
        out.replace_range(start..end, &replacement);
    }
    out
}

const UNFORMATTED: &str = "<?php\nclass A{\npublic function f($a,$b){\nreturn [$a=>$b];\n}\n}\n";

#[test]
fn formats_a_document_with_the_indent_the_client_asks_for() {
    let (mut client, result) = Client::start(json!({}), Value::Null);
    assert_eq!(result["capabilities"]["documentFormattingProvider"], true);
    assert_eq!(result["capabilities"]["documentRangeFormattingProvider"], true);
    client.open(URI, UNFORMATTED);
    client.diagnostics(URI);
    let edits = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": URI }, "options": { "tabSize": 2, "insertSpaces": true } }),
    );
    assert_eq!(
        apply_edits(UNFORMATTED, &edits),
        "<?php\nclass A\n{\n  public function f($a, $b)\n  {\n    return [$a => $b];\n  }\n}\n"
    );
    let edits = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": URI }, "options": { "tabSize": 4, "insertSpaces": false } }),
    );
    assert!(apply_edits(UNFORMATTED, &edits).contains("\n\tpublic function f($a, $b)\n\t{\n\t\treturn"));
    client.shutdown();
}

#[test]
fn leaves_a_document_with_syntax_errors_alone() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    client.open(URI, "<?php\nclass A{\npublic function f(\n");
    client.diagnostics(URI);
    let edits = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": URI }, "options": { "tabSize": 4, "insertSpaces": true } }),
    );
    assert!(edits.is_null());
    client.shutdown();
}

#[test]
fn formats_a_range_and_the_options_come_from_the_settings() {
    let (mut client, _) = Client::start(
        json!({}),
        json!({ "format": { "classBrace": "sameLine", "functionBrace": "sameLine" } }),
    );
    client.open(URI, UNFORMATTED);
    client.diagnostics(URI);
    let edits = client.request(
        "textDocument/rangeFormatting",
        json!({
            "textDocument": { "uri": URI },
            "range": { "start": { "line": 3, "character": 0 }, "end": { "line": 3, "character": 5 } },
            "options": { "tabSize": 4, "insertSpaces": true }
        }),
    );
    assert_eq!(
        apply_edits(UNFORMATTED, &edits),
        "<?php\nclass A{\npublic function f($a,$b){\n        return [$a => $b];\n}\n}\n"
    );
    let edits = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": URI }, "options": { "tabSize": 4, "insertSpaces": true } }),
    );
    assert!(apply_edits(UNFORMATTED, &edits).starts_with("<?php\nclass A {\n    public function f($a, $b) {\n"));
    client.shutdown();
}

#[test]
fn the_client_can_answer_the_format_settings_per_document() {
    let (mut client, _) = Client::start(json!({ "workspace": { "configuration": true } }), Value::Null);
    client.open(URI, UNFORMATTED);
    let request = client.wait_for(|message| match message {
        Message::Request(request) if request.method == "workspace/configuration" => Some(request.clone()),
        _ => None,
    });
    client.send(Response::new_ok(
        request.id,
        json!([{ "format": { "alignArrayArrows": true, "lineLength": 0 } }]),
    ));
    let text = "<?php\n$a = [\n'x' => 1,\n'yyy' => 2,\n];\n";
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": URI, "version": 2 }, "contentChanges": [{ "text": text }] }),
    );
    let edits = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": URI }, "options": { "tabSize": 4, "insertSpaces": true } }),
    );
    assert_eq!(
        apply_edits(text, &edits),
        "<?php\n$a = [\n    'x'   => 1,\n    'yyy' => 2,\n];\n"
    );
    client.shutdown();
}

#[test]
fn formats_the_line_a_typed_brace_or_newline_belongs_to() {
    let (mut client, result) = Client::start(json!({}), Value::Null);
    assert_eq!(
        result["capabilities"]["documentOnTypeFormattingProvider"]["moreTriggerCharacter"],
        json!(["}", ";"])
    );
    let text = "<?php\nfunction a()\n{\nif ($x) {\nfoo();\n}\n}\n";
    client.open(URI, text);
    client.diagnostics(URI);
    let on_type = |client: &mut Client, line: u32, character: u32, ch: &str| {
        client.request(
            "textDocument/onTypeFormatting",
            json!({
                "textDocument": { "uri": URI },
                "position": { "line": line, "character": character },
                "ch": ch,
                "options": { "tabSize": 4, "insertSpaces": true }
            }),
        )
    };
    let edits = on_type(&mut client, 5, 1, "}");
    assert_eq!(
        apply_edits(text, &edits),
        "<?php\nfunction a()\n{\nif ($x) {\nfoo();\n    }\n}\n"
    );
    let edits = on_type(&mut client, 4, 6, ";");
    assert_eq!(
        apply_edits(text, &edits),
        "<?php\nfunction a()\n{\nif ($x) {\n        foo();\n}\n}\n"
    );

    let unfinished = "<?php\nclass A\n{\n    public function f()\n    {\n\n";
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": URI, "version": 2 }, "contentChanges": [{ "text": unfinished }] }),
    );
    let edits = on_type(&mut client, 5, 0, "\n");
    assert_eq!(
        apply_edits(unfinished, &edits),
        "<?php\nclass A\n{\n    public function f()\n    {\n        \n"
    );
    client.shutdown();
}

#[test]
fn usages_in_the_installed_packages_are_found_when_asked_for() {
    let disk = Disk::new();
    disk.write(
        "project/vendor/acme/lib/src/Factory.php",
        "<?php\nnamespace Acme\\Lib;\n\nclass Factory\n{\n    public function make(): Widget\n    {\n        return new Widget();\n    }\n}\n",
    );
    disk.write(
        "project/src/Page.php",
        "<?php\nnamespace App;\n\nuse Acme\\Lib\\Widget;\n\nfunction page(Widget $widget) {}\n",
    );
    let page = disk.uri("project/src/Page.php");
    let text = std::fs::read_to_string(disk.path("project/src/Page.php")).expect("read");
    let files = |found: &Value| {
        let mut names: Vec<String> = found
            .as_array()
            .expect("locations")
            .iter()
            .map(|location| {
                location["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        names.sort();
        names.dedup();
        names
    };
    let ask = |client: &mut Client| {
        client.request(
            "textDocument/references",
            json!({
                "textDocument": { "uri": page },
                "position": { "line": 5, "character": 16 },
                "context": { "includeDeclaration": false }
            }),
        )
    };

    let mut client = indexed_server(&disk);
    client.open(&page, &text);
    assert_eq!(files(&ask(&mut client)), ["Page.php"]);
    client.shutdown();

    let mut options = disk.options();
    options["usages"] = json!({ "packages": true });
    let (mut client, _) = Client::start_in(PROGRESS_CAPABILITIES(), options, json!(disk.uri("project")));
    client.wait_for_indexing();
    client.open(&page, &text);
    assert_eq!(files(&ask(&mut client)), ["Factory.php", "Page.php"]);
    client.shutdown();
}
