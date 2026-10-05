//! Talks to the server over an in-memory connection, the way an editor talks to it over stdio.
#![allow(dead_code)]

use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::{Value, json};

pub const TIMEOUT: Duration = Duration::from_secs(10);

pub struct Client {
    pub connection: Connection,
    pub server: Option<JoinHandle<()>>,
    pub next_id: i32,
    /// Notifications and requests seen while waiting for something else.
    pub backlog: Vec<Message>,
}

impl Client {
    pub fn start(capabilities: Value, options: Value) -> (Client, Value) {
        Client::start_in(capabilities, options, Value::Null)
    }

    pub fn start_in(capabilities: Value, options: Value, root_uri: Value) -> (Client, Value) {
        let (server_side, client_side) = Connection::memory();
        let server = std::thread::spawn(move || {
            php_language_server::run(server_side).expect("the server runs to the end");
        });
        let mut client = Client {
            connection: client_side,
            server: Some(server),
            next_id: 0,
            backlog: Vec::new(),
        };
        let result = client.request(
            "initialize",
            json!({ "processId": null, "rootUri": root_uri, "capabilities": capabilities, "initializationOptions": options }),
        );
        client.notify("initialized", json!({}));
        (client, result)
    }

    pub fn send(&self, message: impl Into<Message>) {
        self.connection
            .sender
            .send(message.into())
            .expect("the server is listening");
    }

    /// The message of the error a request answers with.
    pub fn request_error(&mut self, method: &str, params: Value) -> String {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.send(Request::new(id.clone(), method.to_string(), params));
        loop {
            match self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .expect("the server answers")
            {
                Message::Response(response) if response.id == id => {
                    return response.response_result.expect_err("the request fails").message;
                }
                other => self.backlog.push(other),
            }
        }
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(Notification::new(method.to_string(), params));
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.send(Request::new(id.clone(), method.to_string(), params));
        loop {
            match self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .expect("the server answers")
            {
                Message::Response(response) if response.id == id => {
                    return response.response_result.expect("the request succeeds");
                }
                other => self.backlog.push(other),
            }
        }
    }

    /// The next message that `pick` accepts, from the backlog first.
    pub fn wait_for<T>(&mut self, mut pick: impl FnMut(&Message) -> Option<T>) -> T {
        if let Some(position) = self.backlog.iter().position(|message| pick(message).is_some()) {
            let message = self.backlog.remove(position);
            return pick(&message).expect("picked above");
        }
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .expect("a message arrives");
            if let Some(found) = pick(&message) {
                return found;
            }
            self.backlog.push(message);
        }
    }

    pub fn open(&self, uri: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": "php", "version": 1, "text": text } }),
        );
    }

    /// The next diagnostics published for a document.
    pub fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        self.wait_for(|message| match message {
            Message::Notification(notification) if notification.method == "textDocument/publishDiagnostics" => {
                (notification.params["uri"] == uri).then(|| {
                    notification.params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
            }
            _ => None,
        })
    }

    pub fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        self.server
            .take()
            .expect("started")
            .join()
            .expect("the server stops cleanly");
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

impl Client {
    /// Answers the request to create a progress and waits until the progress ends, which is when the
    /// project and the stubs are indexed.
    pub fn wait_for_indexing(&mut self) -> Vec<String> {
        let mut kinds = Vec::new();
        loop {
            let message = self.wait_for(|message| match message {
                Message::Request(request) if request.method == "window/workDoneProgress/create" => {
                    Some(Message::Request(request.clone()))
                }
                Message::Notification(notification) if notification.method == "$/progress" => {
                    Some(Message::Notification(notification.clone()))
                }
                _ => None,
            });
            match message {
                Message::Request(request) => self.send(Response::new_ok(request.id, Value::Null)),
                Message::Notification(notification) => {
                    let kind = notification.params["value"]["kind"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let done = kind == "end";
                    kinds.push(kind);
                    if done {
                        return kinds;
                    }
                }
                Message::Response(_) => {}
            }
        }
    }

    pub fn at(&mut self, method: &str, uri: &str, line: u32, character: u32) -> Value {
        self.request(
            method,
            json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
        )
    }
}

pub fn indexed_server(disk: &Disk) -> Client {
    let (mut client, _) = Client::start_in(PROGRESS_CAPABILITIES(), disk.options(), json!(disk.uri("project")));
    let kinds = client.wait_for_indexing();
    assert_eq!(kinds.first().map(String::as_str), Some("begin"), "{kinds:?}");
    client
}
