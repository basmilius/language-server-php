//! Refactors through the protocol: the code actions with their edits, the late resolve of the
//! expensive ones, the rename that follows an extract, and the edits that follow a renamed file.

mod common;

use common::*;
use serde_json::{Value, json};

fn range_at(line: u32, from: u32, to: u32) -> Value {
    json!({ "start": { "line": line, "character": from }, "end": { "line": line, "character": to } })
}

fn action<'a>(actions: &'a Value, title: &str) -> &'a Value {
    actions
        .as_array()
        .expect("a list of actions")
        .iter()
        .find(|action| action["title"] == title)
        .unwrap_or_else(|| panic!("no action '{title}' in {actions}"))
}

const PAGE: &str = "<?php\nclass Page\n{\n    public function total(int $a, int $b): int\n    {\n        return ($a + $b) * 2;\n    }\n}\n";

#[test]
fn extracts_a_variable_and_points_at_its_name() {
    let (mut client, _) = Client::start(
        json!({ "workspace": { "workspaceEdit": { "documentChanges": true } } }),
        Value::Null,
    );
    let uri = "file:///project/page.php";
    client.open(uri, PAGE);
    client.diagnostics(uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(5, 16, 23), "context": { "diagnostics": [] } }),
    );
    let extract = action(&actions, "Extract variable");
    assert_eq!(extract["kind"], "refactor.extract");
    let edits = &extract["edit"]["documentChanges"][0]["edits"];
    assert_eq!(edits[0]["newText"], "        $int = $a + $b;\n");
    assert_eq!(edits[0]["range"]["start"], json!({ "line": 5, "character": 0 }));
    assert_eq!(edits[1]["newText"], "$int");
    assert_eq!(extract["command"]["command"], "php.rename");
    assert_eq!(
        extract["command"]["arguments"][0]["position"],
        json!({ "line": 5, "character": 9 }),
        "after the edit the name sits on the line of the declaration"
    );
    client.shutdown();
}

#[test]
fn a_client_with_snippets_gets_the_name_as_a_placeholder() {
    let (mut client, _) = Client::start(
        json!({ "workspace": { "workspaceEdit": { "documentChanges": true, "snippetEditSupport": true } } }),
        Value::Null,
    );
    let uri = "file:///project/page.php";
    client.open(uri, PAGE);
    client.diagnostics(uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(5, 16, 23), "context": { "diagnostics": [] } }),
    );
    let extract = action(&actions, "Extract variable");
    let first = &extract["edit"]["documentChanges"][0]["edits"][0];
    assert_eq!(first["snippet"]["kind"], "snippet");
    assert_eq!(first["snippet"]["value"], "        \\$${1:int} = \\$a + \\$b;\n");
    assert!(extract["command"].is_null(), "the cursor is in the placeholder");
    client.shutdown();
}

#[test]
fn a_client_that_resolves_gets_the_edit_of_a_refactor_late() {
    let (mut client, _) = Client::start(
        json!({ "textDocument": { "codeAction": { "resolveSupport": { "properties": ["edit"] } } } }),
        Value::Null,
    );
    let uri = "file:///project/page.php";
    client.open(uri, PAGE);
    client.diagnostics(uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(5, 16, 23), "context": { "diagnostics": [], "only": ["refactor.extract"] } }),
    );
    let extract = action(&actions, "Extract variable").clone();
    assert!(extract["edit"].is_null(), "the edit waits for the resolve");
    let resolved = client.request("codeAction/resolve", extract);
    assert_eq!(
        resolved["edit"]["changes"][uri][0]["newText"],
        "        $int = $a + $b;\n"
    );
    assert!(resolved["data"].is_null());
    client.shutdown();
}

#[test]
fn a_refusal_comes_back_as_an_error_with_its_reason() {
    let (mut client, _) = Client::start(
        json!({ "textDocument": { "codeAction": { "resolveSupport": { "properties": ["edit"] } } } }),
        Value::Null,
    );
    let uri = "file:///project/page.php";
    client.open(
        uri,
        "<?php\nfunction f($a, $b)\n{\n    return $a && strlen($b) > 1;\n}\n",
    );
    client.diagnostics(uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(3, 17, 31), "context": { "diagnostics": [] } }),
    );
    let extract = action(&actions, "Extract variable").clone();
    let reason = client.request_error("codeAction/resolve", extract);
    assert_eq!(reason, "The expression only runs when the left side allows it");
    client.shutdown();
}

const CLASS_CAPABILITIES: fn() -> Value = || {
    json!({
        "window": { "workDoneProgress": true },
        "textDocument": { "codeAction": { "resolveSupport": { "properties": ["edit"] } } },
        "workspace": { "workspaceEdit": { "documentChanges": true, "resourceOperations": ["rename"] } }
    })
};

fn project_with_classes() -> Disk {
    let disk = Disk::new();
    disk.write(
        "project/src/Domain/Order.php",
        "<?php\nnamespace App\\Domain;\n\nclass Order {}\n",
    );
    disk.write(
        "project/src/Services/Billing.php",
        "<?php\nnamespace App\\Services;\n\nuse App\\Models\\User;\n\nclass Billing\n{\n    public function charge(User $user): void {}\n}\n",
    );
    disk
}

#[test]
fn moves_a_class_to_another_namespace_with_its_file_and_references() {
    let disk = project_with_classes();
    let (mut client, _) = Client::start_in(CLASS_CAPABILITIES(), disk.options(), json!(disk.uri("project")));
    client.wait_for_indexing();
    let user = disk.uri("project/src/Models/User.php");
    let text = std::fs::read_to_string(disk.path("project/src/Models/User.php")).expect("the class");
    client.open(&user, &text);
    client.diagnostics(&user);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": user }, "range": range_at(4, 8, 8), "context": { "diagnostics": [], "only": ["refactor.move"] } }),
    );
    let titles: Vec<&str> = actions
        .as_array()
        .expect("actions")
        .iter()
        .filter_map(|action| action["title"].as_str())
        .collect();
    assert!(titles.contains(&"Move class to namespace App\\Domain"), "{titles:?}");
    let action = action(&actions, "Move class to namespace App\\Domain").clone();
    assert_eq!(action["kind"], "refactor.move");
    assert!(action["edit"].is_null(), "the edit waits for the resolve");
    let resolved = client.request("codeAction/resolve", action);
    let changes = resolved["documentChanges"]
        .as_array()
        .or(resolved["edit"]["documentChanges"].as_array())
        .expect("document changes");
    let rename = changes
        .iter()
        .find(|change| change["kind"] == "rename")
        .expect("the file moves");
    assert_eq!(rename["oldUri"], user);
    assert_eq!(rename["newUri"], disk.uri("project/src/Domain/User.php"));
    let own = changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == user)
        .expect("edits of the class");
    assert_eq!(own["edits"][0]["newText"], "App\\Domain");
    let billing = changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == disk.uri("project/src/Services/Billing.php"))
        .expect("edits of a file that uses it");
    assert_eq!(billing["edits"][0]["newText"], "App\\Domain\\User");
    let last_edit = changes.iter().rposition(|change| change.get("textDocument").is_some());
    let first_move = changes.iter().position(|change| change["kind"] == "rename");
    assert!(last_edit < first_move, "the edits come before the move");
    client.shutdown();
}

#[test]
fn a_renamed_file_brings_its_namespace_and_references_along() {
    let disk = project_with_classes();
    let (mut client, result) = Client::start_in(CLASS_CAPABILITIES(), disk.options(), json!(disk.uri("project")));
    let filters = &result["capabilities"]["workspace"]["fileOperations"]["willRename"]["filters"];
    assert_eq!(filters[0]["pattern"]["glob"], "**/*.php");
    client.wait_for_indexing();
    let edit = client.request(
        "workspace/willRenameFiles",
        json!({ "files": [{
            "oldUri": disk.uri("project/src/Models/User.php"),
            "newUri": disk.uri("project/src/Domain/User.php"),
        }] }),
    );
    let changes = edit["documentChanges"].as_array().expect("document changes");
    assert!(
        changes.iter().all(|change| change.get("kind").is_none()),
        "the client moves the file itself"
    );
    let own = changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == disk.uri("project/src/Models/User.php"))
        .expect("edits of the moved class, at its old place");
    assert_eq!(own["edits"][0]["newText"], "App\\Domain");
    let billing = changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == disk.uri("project/src/Services/Billing.php"))
        .expect("edits of a file that uses it");
    assert_eq!(billing["edits"][0]["newText"], "App\\Domain\\User");
    let unrelated = client.request(
        "workspace/willRenameFiles",
        json!({ "files": [{
            "oldUri": disk.uri("project/stubs/redis/redis.php"),
            "newUri": disk.uri("project/stubs/redis/other.php"),
        }] }),
    );
    assert!(
        unrelated.is_null(),
        "a file that is no class of the project changes nothing"
    );
    client.shutdown();
}

#[test]
fn a_moved_folder_moves_every_class_in_it() {
    let disk = project_with_classes();
    disk.write(
        "project/src/Models/Team.php",
        "<?php\nnamespace App\\Models;\n\nclass Team {}\n",
    );
    let (mut client, _) = Client::start_in(CLASS_CAPABILITIES(), disk.options(), json!(disk.uri("project")));
    client.wait_for_indexing();
    let edit = client.request(
        "workspace/willRenameFiles",
        json!({ "files": [{
            "oldUri": disk.uri("project/src/Models"),
            "newUri": disk.uri("project/src/Entities"),
        }] }),
    );
    let changes = edit["documentChanges"].as_array().expect("document changes");
    let touched: Vec<&str> = changes
        .iter()
        .filter_map(|change| change["textDocument"]["uri"].as_str())
        .collect();
    assert!(
        touched.contains(&disk.uri("project/src/Models/User.php").as_str()),
        "{touched:?}"
    );
    assert!(
        touched.contains(&disk.uri("project/src/Models/Team.php").as_str()),
        "{touched:?}"
    );
    assert!(
        touched.contains(&disk.uri("project/src/Services/Billing.php").as_str()),
        "{touched:?}"
    );
    client.shutdown();
}

#[test]
fn a_rewrite_is_offered_with_its_edit_and_formatted_on_its_lines() {
    let (mut client, _) = Client::start(json!({}), Value::Null);
    let uri = "file:///project/page.php";
    client.open(
        uri,
        "<?php\nfunction f($a)\n{\n    $v = array(1, 2);\n    return $v;\n}\n",
    );
    client.diagnostics(uri);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": uri }, "range": range_at(3, 12, 12), "context": { "diagnostics": [], "only": ["refactor.rewrite"] } }),
    );
    let rewrite = action(&actions, "Convert array() to []");
    assert_eq!(rewrite["kind"], "refactor.rewrite");
    let edits = &rewrite["edit"]["changes"][uri];
    assert_eq!(edits[0]["newText"], "[");
    assert_eq!(edits[1]["newText"], "]");
    client.shutdown();
}

#[test]
fn the_editorconfig_of_a_project_sets_the_indent_and_the_line_length() {
    let disk = Disk::new();
    disk.write(
        "project/.editorconfig",
        "root = true\n\n[*.php]\nindent_style = tab\nmax_line_length = 60\n",
    );
    let (mut client, _) = Client::start_in(json!({}), disk.options(), json!(disk.uri("project")));
    let uri = disk.uri("project/src/Page.php");
    client.open(
        &uri,
        "<?php\nfunction f()\n{\n    return strlen('aaaaaaaaaaaaaaaaaaaa') + strlen('bbbbbbbbbbbbbbbbbbbbbbb') + strlen('cccc');\n}\n",
    );
    client.diagnostics(&uri);
    let edits = client.request(
        "textDocument/formatting",
        json!({ "textDocument": { "uri": uri }, "options": { "tabSize": 4, "insertSpaces": true } }),
    );
    let texts: Vec<&str> = edits
        .as_array()
        .expect("edits")
        .iter()
        .filter_map(|edit| edit["newText"].as_str())
        .collect();
    assert!(texts.iter().any(|text| text.contains('\t')), "{texts:?}");
    assert!(
        texts.iter().any(|text| text.contains('\n')),
        "the long line is broken: {texts:?}"
    );
    client.shutdown();
}

#[test]
fn extracts_an_interface_into_a_file_of_its_own() {
    let disk = project_with_classes();
    let capabilities = json!({
        "window": { "workDoneProgress": true },
        "textDocument": { "codeAction": { "resolveSupport": { "properties": ["edit"] } } },
        "workspace": { "workspaceEdit": { "documentChanges": true, "resourceOperations": ["create", "rename"] } }
    });
    let (mut client, _) = Client::start_in(capabilities, disk.options(), json!(disk.uri("project")));
    client.wait_for_indexing();
    let user = disk.uri("project/src/Models/User.php");
    let text = std::fs::read_to_string(disk.path("project/src/Models/User.php")).expect("the class");
    client.open(&user, &text);
    client.diagnostics(&user);
    let actions = client.request(
        "textDocument/codeAction",
        json!({ "textDocument": { "uri": user }, "range": range_at(4, 8, 8), "context": { "diagnostics": [], "only": ["refactor.extract"] } }),
    );
    let action = action(&actions, "Extract interface UserInterface").clone();
    let resolved = if action["edit"].is_null() {
        client.request("codeAction/resolve", action)
    } else {
        action
    };
    let changes = resolved["documentChanges"]
        .as_array()
        .or(resolved["edit"]["documentChanges"].as_array())
        .unwrap_or_else(|| panic!("{resolved}"));
    let interface = disk.uri("project/src/Models/UserInterface.php");
    let create = changes
        .iter()
        .position(|change| change["kind"] == "create")
        .expect("the file is made");
    assert_eq!(changes[create]["uri"], interface);
    let written = &changes[create + 1];
    assert_eq!(written["textDocument"]["uri"], interface);
    let body = written["edits"][0]["newText"].as_str().expect("the interface");
    assert!(body.contains("interface UserInterface\n{\n    /** Finds a user. */\n    public static function find(int $id): ?static;\n\n    public function posts(): array;\n}\n"), "{body}");
    let own = changes
        .iter()
        .find(|change| change["textDocument"]["uri"] == user)
        .expect("the class implements it");
    assert!(
        own["edits"][0]["newText"]
            .as_str()
            .is_some_and(|text| text.contains("implements UserInterface")),
        "{own}"
    );
    client.shutdown();
}
