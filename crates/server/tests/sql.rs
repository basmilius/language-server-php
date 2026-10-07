//! SQL in the strings of PHP documents, over an in-memory connection the way an editor talks to the
//! server: what is read as SQL, and every answer for it in the positions of the PHP document.

mod common;

use common::*;
use serde_json::{Value, json};

const SCHEMA: &str = "CREATE TABLE orgs (id int PRIMARY KEY, title varchar(100));\nCREATE TABLE users (id int PRIMARY KEY, org_id int REFERENCES orgs (id), email varchar(255) NOT NULL, name varchar(100), status varchar(10));\n";

/// A project with the PDO stub and the DDL of its tables in a `.sql` file.
fn disk() -> Disk {
    let disk = Disk::new();
    disk.write(
        "stubs/PDO/PDO.php",
        "<?php\nclass PDO {\n    public function query(string $query, ?int $fetchMode = null) {}\n    public function prepare(string $query, array $options = []) {}\n    public function exec(string $statement) {}\n}\n",
    );
    disk.write("project/db/schema.sql", SCHEMA);
    disk
}

/// The line and UTF-16 character of the first `needle` in `text`, moved `delta` bytes on.
fn at(text: &str, needle: &str, delta: usize) -> (u32, u32) {
    let offset = text.find(needle).unwrap_or_else(|| panic!("{needle} in {text}")) + delta;
    let before = &text[..offset];
    let line = before.matches('\n').count() as u32;
    let character = before.rsplit('\n').next().unwrap_or_default().encode_utf16().count() as u32;
    (line, character)
}

fn position(text: &str, needle: &str, delta: usize) -> Value {
    let (line, character) = at(text, needle, delta);
    json!({ "line": line, "character": character })
}

/// The text a range of a document covers, for a document of one line per range.
fn covered(text: &str, range: &Value) -> String {
    let line = range["start"]["line"].as_u64().expect("a line") as usize;
    let start = range["start"]["character"].as_u64().expect("a start") as usize;
    let end = range["end"]["character"].as_u64().expect("an end") as usize;
    text.lines().nth(line).expect("the line")[start..end].to_string()
}

/// Waits until the diagnostics of a document hold one of SQL, and gives the SQL ones.
fn sql_diagnostics(client: &mut Client, uri: &str) -> Vec<Value> {
    loop {
        let found: Vec<Value> = client
            .diagnostics(uri)
            .into_iter()
            .filter(|diagnostic| diagnostic["source"] == "sql")
            .collect();
        if !found.is_empty() {
            return found;
        }
    }
}

fn started(disk: &Disk, capabilities: Value, options: Value) -> Client {
    let mut options = options;
    for (key, value) in disk.options().as_object().expect("options") {
        options[key] = value.clone();
    }
    let (mut client, _) = Client::start_in(capabilities, options, json!(disk.uri("project")));
    client.wait_for_indexing();
    client
}

const PAGE: &str = "<?php\nfunction users(PDO $pdo, int $id) {\n    $pdo->query('SELECT emial FROM users WHERE id = ' . $id);\n    $statement = $pdo->prepare(\"SELECT u.name, o.title FROM users u JOIN orgs o ON o.id = u.org_id WHERE u.status = 'active'\");\n    echo 'Select a file';\n}\n";

#[test]
fn reports_the_problems_of_the_sql_in_a_string_where_the_string_has_them() {
    let disk = disk();
    let mut client = started(&disk, PROGRESS_CAPABILITIES(), json!({}));
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    let found = sql_diagnostics(&mut client, &uri);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["code"], "unresolved-column");
    assert_eq!(found[0]["source"], "sql");
    assert_eq!(covered(PAGE, &found[0]["range"]), "emial");
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": found[0]["range"],
            "context": { "diagnostics": [found[0]] }
        }),
    );
    let fix = actions
        .as_array()
        .expect("actions")
        .iter()
        .find(|action| action["title"].as_str().is_some_and(|title| title.contains("email")))
        .expect("a quick fix");
    assert_eq!(fix["kind"], "quickfix");
    assert_eq!(fix["diagnostics"][0]["code"], "unresolved-column");
    let edits = fix["edit"]["changes"][&uri].as_array().expect("edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["newText"], "email");
    assert_eq!(covered(PAGE, &edits[0]["range"]), "emial");
    client.shutdown();
}

#[test]
fn completes_hovers_and_navigates_inside_a_string() {
    let disk = disk();
    let mut client = started(&disk, PROGRESS_CAPABILITIES(), json!({}));
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    sql_diagnostics(&mut client, &uri);

    let (line, character) = at(PAGE, "u.name", 2);
    let list = client.request(
        "textDocument/completion",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "triggerKind": 2, "triggerCharacter": "." }
        }),
    );
    let labels: Vec<&str> = list["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter_map(|item| item["label"].as_str())
        .collect();
    assert!(labels.contains(&"email") && labels.contains(&"org_id"), "{labels:?}");
    assert!(!labels.contains(&"title"), "only the columns of the alias: {labels:?}");

    let (line, character) = at(PAGE, "    echo", 0);
    let nothing = client.request(
        "textDocument/completion",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character + 4 },
            "context": { "triggerKind": 2, "triggerCharacter": " " }
        }),
    );
    assert!(nothing.is_null(), "a space completes nothing in PHP: {nothing}");

    let (line, character) = at(PAGE, "o.title", 3);
    let hover = client.at("textDocument/hover", &uri, line, character);
    let text = hover["contents"]["value"].as_str().expect("markdown");
    assert!(text.contains("title") && text.contains("varchar(100)"), "{text}");
    assert_eq!(covered(PAGE, &hover["range"]), "title");

    let definition = client.at("textDocument/definition", &uri, line, character);
    let target = &definition[0];
    assert_eq!(target["uri"], disk.uri("project/db/schema.sql"));
    assert_eq!(covered(SCHEMA, &target["range"]), "title");

    let (line, character) = at(PAGE, "users u", 7);
    let highlights = client.at("textDocument/documentHighlight", &uri, line, character);
    let spans: Vec<String> = highlights
        .as_array()
        .expect("highlights")
        .iter()
        .map(|highlight| covered(PAGE, &highlight["range"]))
        .collect();
    assert_eq!(spans, ["u", "u", "u", "u"]);
    client.shutdown();
}

#[test]
fn renames_an_alias_within_its_string_and_refuses_a_table() {
    let disk = disk();
    let mut client = started(&disk, PROGRESS_CAPABILITIES(), json!({}));
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    sql_diagnostics(&mut client, &uri);
    let (line, character) = at(PAGE, "o.title", 0);
    let prepared = client.at("textDocument/prepareRename", &uri, line, character);
    assert_eq!(prepared["placeholder"], "o");
    let edit = client.request(
        "textDocument/rename",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character }, "newName": "org" }),
    );
    let edits = edit["changes"][&uri].as_array().expect("edits");
    assert_eq!(edits.len(), 3);
    assert!(edits.iter().all(|edit| edit["newText"] == "org"));
    let (line, character) = at(PAGE, "JOIN orgs", 6);
    let refused = client.request_error(
        "textDocument/prepareRename",
        json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
    );
    assert!(refused.contains("belongs to the schema"), "{refused}");
    client.shutdown();
}

#[test]
fn finds_a_table_in_every_string_and_in_the_sql_files() {
    let disk = disk();
    let mut client = started(&disk, PROGRESS_CAPABILITIES(), json!({}));
    let uri = disk.uri("project/src/Page.php");
    client.open(&uri, PAGE);
    sql_diagnostics(&mut client, &uri);
    let (line, character) = at(PAGE, "FROM users", 6);
    let found = client.request(
        "textDocument/references",
        json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
            "context": { "includeDeclaration": true }
        }),
    );
    let places: Vec<(String, u64)> = found
        .as_array()
        .expect("locations")
        .iter()
        .map(|location| {
            (
                location["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string(),
                location["range"]["start"]["line"].as_u64().unwrap_or_default(),
            )
        })
        .collect();
    assert!(places.contains(&("Page.php".to_string(), 2)), "{places:?}");
    assert!(places.contains(&("Page.php".to_string(), 3)), "{places:?}");
    assert!(places.iter().any(|(file, _)| file == "schema.sql"), "{places:?}");
    client.shutdown();
}

#[test]
fn colors_the_sql_of_a_string_without_overlapping_tokens() {
    let disk = disk();
    let (mut client, result) = Client::start_in(PROGRESS_CAPABILITIES(), disk.options(), json!(disk.uri("project")));
    client.wait_for_indexing();
    let legend = &result["capabilities"]["semanticTokensProvider"]["legend"];
    let types: Vec<&str> = legend["tokenTypes"]
        .as_array()
        .expect("types")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(types[0], "namespace", "PHP's types come first");
    for name in ["keyword", "string", "number", "operator", "comment", "type"] {
        assert!(types.contains(&name), "{name} in {types:?}");
    }
    let uri = disk.uri("project/src/Page.php");
    let text = "<?php\n/* language=SQL */\n$sql = \"SELECT count(*) FROM users WHERE status = 'on' AND id > 10 AND name = {$name}\";\n";
    client.open(&uri, text);
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
    let (mut line, mut start) = (0u64, 0u64);
    let mut last_end = 0u64;
    for token in data.chunks(5) {
        line += token[0];
        start = if token[0] == 0 { start + token[1] } else { token[1] };
        if token[0] == 0 {
            assert!(start >= last_end, "tokens do not overlap");
        }
        last_end = start + token[2];
        let line_text = text.lines().nth(line as usize).expect("a line");
        decoded.push(format!(
            "{} {}",
            &line_text[start as usize..(start + token[2]) as usize],
            types[token[3] as usize]
        ));
    }
    for expected in [
        "SELECT keyword",
        "count function",
        "FROM keyword",
        "WHERE keyword",
        "'on' string",
        "10 number",
        "$name variable",
    ] {
        assert!(
            decoded.iter().any(|token| token == expected),
            "{expected} in {decoded:?}"
        );
    }
    client.shutdown();
}

#[test]
fn offers_signature_help_and_inlay_hints_of_sql() {
    let disk = disk();
    let mut client = started(&disk, PROGRESS_CAPABILITIES(), json!({}));
    let uri = disk.uri("project/src/Page.php");
    let text = "<?php\nfunction add(PDO $pdo) {\n    $pdo->exec(\"INSERT INTO orgs VALUES (1, nullif(NULL, 'x'))\");\n    $pdo->query('SELECT nope FROM orgs');\n}\n";
    client.open(&uri, text);
    sql_diagnostics(&mut client, &uri);
    let (line, character) = at(text, "NULL, ", 6);
    let help = client.at("textDocument/signatureHelp", &uri, line, character);
    let label = help["signatures"][0]["label"].as_str().expect("a signature");
    assert!(label.to_ascii_lowercase().contains("nullif"), "{label}");
    assert_eq!(help["activeParameter"], 1);
    let hints = client.request(
        "textDocument/inlayHint",
        json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 4, "character": 0 } } }),
    );
    let labels: Vec<&str> = hints
        .as_array()
        .expect("hints")
        .iter()
        .filter_map(|hint| hint["label"].as_str())
        .collect();
    assert!(labels.contains(&"id:") && labels.contains(&"title:"), "{labels:?}");
    client.shutdown();
}

#[test]
fn follows_the_settings_and_a_snapshot_that_changes() {
    let disk = disk();
    disk.write(
        "project/schema.json",
        r#"{ "formatVersion": 1, "schemas": [{ "name": "public", "tables": [{ "name": "people", "columns": [{ "name": "id" }] }] }] }"#,
    );
    let mut client = started(
        &disk,
        json!({ "window": { "workDoneProgress": true }, "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": true } } }),
        json!({ "sql": { "dialect": "postgres", "schema": "schema.json" } }),
    );
    let uri = disk.uri("project/src/Page.php");
    let text = "<?php\n/* language=SQL */\n$a = 'SELECT nope FROM people';\n";
    client.open(&uri, text);
    let found = sql_diagnostics(&mut client, &uri);
    assert_eq!(found[0]["code"], "unresolved-column");
    assert_eq!(covered(text, &found[0]["range"]), "nope");

    disk.write(
        "project/schema.json",
        r#"{ "formatVersion": 1, "schemas": [{ "name": "public", "tables": [{ "name": "people", "columns": [{ "name": "id" }, { "name": "nope" }] }] }] }"#,
    );
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": disk.uri("project/schema.json"), "type": 2 }] }),
    );
    loop {
        let sql: Vec<Value> = client
            .diagnostics(&uri)
            .into_iter()
            .filter(|diagnostic| diagnostic["source"] == "sql")
            .collect();
        if sql.is_empty() {
            break;
        }
    }

    client.notify(
        "workspace/didChangeConfiguration",
        json!({ "settings": { "phpLanguageServer": { "sql": { "enabled": false } } } }),
    );
    let hover_at = at(text, "people", 1);
    loop {
        let all = client.diagnostics(&uri);
        if all.iter().all(|diagnostic| diagnostic["source"] != "sql") {
            break;
        }
    }
    let hover = client.at("textDocument/hover", &uri, hover_at.0, hover_at.1);
    assert!(hover.is_null(), "{hover}");
    client.shutdown();
}

#[test]
fn reads_a_heredoc_past_its_indentation_and_finds_nothing_in_interface_text() {
    let disk = disk();
    let mut client = started(&disk, PROGRESS_CAPABILITIES(), json!({}));
    let uri = disk.uri("project/src/Page.php");
    let text = "<?php\nfunction report(PDO $pdo) {\n    $sql = <<<SQL\n        SELECT id, nmae\n          FROM orgs\n        SQL;\n    $pdo->query($sql);\n    $label = 'Select a file';\n    $title = \"Update failed\";\n}\n";
    client.open(&uri, text);
    let found = sql_diagnostics(&mut client, &uri);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["range"]["start"], position(text, "nmae", 0));
    let tokens = client.request(
        "textDocument/semanticTokens/range",
        json!({ "textDocument": { "uri": uri }, "range": { "start": { "line": 7, "character": 0 }, "end": { "line": 9, "character": 0 } } }),
    );
    assert_eq!(
        tokens["data"].as_array().map(Vec::len),
        Some(10),
        "only the tokens of the two variables: {tokens}"
    );
    client.shutdown();
}

/// The SQL diagnostics a client that pulls gets for a document now.
fn pulled(client: &mut Client, uri: &str) -> Vec<Value> {
    let report = client.request("textDocument/diagnostic", json!({ "textDocument": { "uri": uri } }));
    report["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|diagnostic| diagnostic["source"] == "sql")
        .cloned()
        .collect()
}

/// Pulls until the SQL diagnostics of a document satisfy `wanted`: the `.sql` files are read in
/// the background, and their arrival is what changes the answer.
fn pulled_until(client: &mut Client, uri: &str, wanted: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    loop {
        let found = pulled(client, uri);
        if wanted(&found) {
            return found;
        }
    }
}

#[test]
fn reads_a_changed_sql_file_again_and_follows_the_dialect_of_a_path() {
    let disk = disk();
    let mut client = started(
        &disk,
        json!({ "window": { "workDoneProgress": true }, "textDocument": { "diagnostic": {} } }),
        json!({ "sql": { "overrides": [{ "path": "src/Pg", "dialect": "postgres" }] } }),
    );
    let text = "<?php\n/* language=SQL */\n$a = 'SELECT `name`, phone FROM users';\n";
    let plain = disk.uri("project/src/Page.php");
    let postgres = disk.uri("project/src/Pg/Page.php");
    client.open(&plain, text);
    client.open(&postgres, text);
    let found = pulled_until(&mut client, &plain, |found| !found.is_empty());
    let codes: Vec<&str> = found.iter().filter_map(|found| found["code"].as_str()).collect();
    assert_eq!(codes, ["unresolved-column"], "{found:?}");
    assert_eq!(covered(text, &found[0]["range"]), "phone");
    let in_postgres = pulled(&mut client, &postgres);
    assert!(
        in_postgres
            .iter()
            .any(|found| covered(text, &found["range"]) == "`name`"),
        "PostgreSQL has no backticks: {in_postgres:?}"
    );

    disk.write(
        "project/db/schema.sql",
        &SCHEMA.replace("status varchar(10)", "status varchar(10), phone text"),
    );
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": disk.uri("project/db/schema.sql"), "type": 2 }] }),
    );
    assert!(pulled(&mut client, &plain).is_empty(), "the new column is known");
    client.shutdown();
}

#[test]
fn applies_every_safe_fix_of_the_sql_of_a_document_at_once() {
    let disk = disk();
    let mut client = started(&disk, PROGRESS_CAPABILITIES(), json!({}));
    let uri = disk.uri("project/src/Page.php");
    let text = "<?php\n/* language=SQL */\n$a = 'SELECT id FROM users WHERE name = NULL';\n/* language=SQL */\n$b = \"DELETE FROM orgs WHERE title != NULL\";\n";
    client.open(&uri, text);
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
            "context": { "diagnostics": [], "only": ["source.fixAll"] }
        }),
    );
    let actions = actions.as_array().expect("actions");
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["kind"], "source.fixAll.sql");
    let edits = actions[0]["edit"]["changes"][&uri].as_array().expect("edits");
    let written: Vec<&str> = edits.iter().filter_map(|edit| edit["newText"].as_str()).collect();
    assert_eq!(written.len(), 2, "{edits:?}");
    assert!(written.iter().all(|text| text.contains("IS")), "{written:?}");
    client.shutdown();
}
