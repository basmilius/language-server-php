//! Talks to the server over an in-memory connection, the way an editor talks to it over stdio.
#![allow(dead_code)]

use serde_json::{Value, json};

pub const TIMEOUT: std::time::Duration = lsc_server::testing::TIMEOUT;

pub type Client = lsc_server::testing::TestClient;

/// What the tests of this server add to the shared client.
pub trait PhpClient {
    fn start(capabilities: Value, options: Value) -> (Client, Value);
    fn start_in(capabilities: Value, options: Value, root_uri: Value) -> (Client, Value);
    fn open(&self, uri: &str, text: &str);
    /// Answers the request to create a progress and waits until the progress ends, which is when
    /// the project and the stubs are indexed.
    fn wait_for_indexing(&mut self) -> Vec<String>;
}

impl PhpClient for Client {
    fn start(capabilities: Value, options: Value) -> (Client, Value) {
        Client::start_in(capabilities, options, Value::Null)
    }

    fn start_in(capabilities: Value, options: Value, root_uri: Value) -> (Client, Value) {
        Client::connect(
            php_language_server::run,
            json!({ "processId": null, "rootUri": root_uri, "capabilities": capabilities, "initializationOptions": options }),
        )
    }

    fn open(&self, uri: &str, text: &str) {
        self.open_document(uri, "php", text);
    }

    fn wait_for_indexing(&mut self) -> Vec<String> {
        self.wait_for_progress_end()
    }
}

pub struct Disk {
    pub dir: tempfile::TempDir,
}

impl Disk {
    /// A project with Composer metadata and one installed package, a storage folder and a few stubs.
    pub fn new() -> Disk {
        let disk = Disk {
            dir: tempfile::tempdir().expect("a temp dir"),
        };
        disk.write(
            "project/composer.json",
            r#"{
  "require": { "php": "^8.1", "ext-redis": "*" },
  "config": { "platform": { "php": "8.1" } },
  "autoload": { "psr-4": { "App\\": "src/" } }
}"#,
        );
        disk.write(
            "project/src/Models/User.php",
            "<?php\nnamespace App\\Models;\n\n/** A person who can log in. */\nclass User\n{\n    public string $name = '';\n\n    /** Finds a user. */\n    public static function find(int $id): ?static\n    {\n        return null;\n    }\n\n    public function posts(): array\n    {\n        return [];\n    }\n}\n",
        );
        disk.write(
            "project/vendor/composer/installed.json",
            r#"{"packages":[{"name":"acme/lib","install-path":"../acme/lib","autoload":{"psr-4":{"Acme\\Lib\\":"src/"}}}]}"#,
        );
        disk.write(
            "project/vendor/acme/lib/src/Widget.php",
            "<?php\nnamespace Acme\\Lib;\n\nclass Widget\n{\n    public function render(): string\n    {\n        return '';\n    }\n}\n",
        );
        disk.write(
            "stubs/standard/basic.php",
            "<?php\nfunction strlen(string $string): int {}\n\n/** @since 8.4 */\nfunction array_find(array $array, callable $callback): mixed {}\n",
        );
        disk.write(
            "stubs/redis/redis.php",
            "<?php\nclass Redis { public function get(string $key): mixed {} }\n",
        );
        disk.write("stubs/swoole/swoole.php", "<?php\nclass SwooleServer {}\n");
        disk
    }

    pub fn write(&self, relative: &str, text: &str) {
        let path = self.dir.path().join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("created");
        std::fs::write(path, text).expect("written");
    }

    pub fn path(&self, relative: &str) -> std::path::PathBuf {
        self.dir.path().join(relative)
    }

    pub fn uri(&self, relative: &str) -> String {
        format!("file://{}", self.path(relative).display())
    }

    pub fn options(&self) -> Value {
        json!({
            "storagePath": self.path("storage"),
            "stubsPath": self.path("stubs"),
        })
    }
}

pub const PROGRESS_CAPABILITIES: fn() -> Value = || json!({ "window": { "workDoneProgress": true } });

pub fn indexed_server(disk: &Disk) -> Client {
    let (mut client, _) = Client::start_in(PROGRESS_CAPABILITIES(), disk.options(), json!(disk.uri("project")));
    let kinds = client.wait_for_indexing();
    assert_eq!(kinds.first().map(String::as_str), Some("begin"), "{kinds:?}");
    client
}
