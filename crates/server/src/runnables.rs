//! What a test file can run, for a client that runs tests: the custom request `php/runnables` and
//! the same list as code lenses. The server runs nothing; the lens carries the `php.runTest` command
//! and the runnable, and the client does the rest.

use std::path::{Path, PathBuf};

use lsp_types::{CodeLens, CodeLensParams, Command, Uri};
use php_analysis::runnables::{Runnable, RunnableKind, RunnableScope, runnables};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::convert::Mapper;
use crate::paths::uri_to_path;
use crate::server::Server;

/// The request that lists what a document can run.
pub(crate) const RUNNABLES_METHOD: &str = "php/runnables";

/// The command a code lens runs. The client implements it.
pub(crate) const RUN_COMMAND: &str = "php.runTest";

/// The names PHPUnit looks for, nearest folder first.
const CONFIG_FILES: &[&str] = &["phpunit.xml", "phpunit.xml.dist", "phpunit.dist.xml"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunnablesParams {
    uri: Option<Uri>,
    text_document: Option<lsp_types::TextDocumentIdentifier>,
}

impl RunnablesParams {
    fn uri(self) -> Option<Uri> {
        self.uri.or(self.text_document.map(|document| document.uri))
    }
}

impl Server<'_> {
    pub(crate) fn runnables(&mut self, params: RunnablesParams) -> Option<Vec<Value>> {
        self.runnables_of(&params.uri()?)
    }

    pub(crate) fn code_lens(&mut self, params: CodeLensParams) -> Option<Vec<CodeLens>> {
        let found = self.runnables_of(&params.text_document.uri)?;
        Some(
            found
                .into_iter()
                .map(|runnable| CodeLens {
                    range: serde_json::from_value(runnable["range"].clone()).unwrap_or_default(),
                    command: Some(Command {
                        title: lens_title(&runnable),
                        command: RUN_COMMAND.to_string(),
                        arguments: Some(vec![runnable]),
                    }),
                    data: None,
                })
                .collect(),
        )
    }

    fn runnables_of(&mut self, uri: &Uri) -> Option<Vec<Value>> {
        let path = uri_to_path(uri)?;
        self.sync_symbols(uri);
        let encoding = self.encoding;
        let document = self.documents.get_mut(uri)?;
        let root = document.parse().syntax();
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        let project = self.workspace.project_for(&path);
        let _document = php_analysis::document::enter(Some(&path));
        let found = runnables(&project.index, &root);
        let folder = self.workspace.folder_of(&path).unwrap_or_else(|| project.root.clone());
        let config = config_file(&path, &folder);
        Some(
            found
                .iter()
                .map(|runnable| runnable_json(runnable, &mapper, &path, config.as_deref()))
                .collect(),
        )
    }
}

fn runnable_json(runnable: &Runnable, mapper: &Mapper<'_>, file: &Path, config: Option<&Path>) -> Value {
    json!({
        "kind": match runnable.kind {
            RunnableKind::PhpUnit => "phpunit",
            RunnableKind::Pest => "pest",
            RunnableKind::Artisan => "artisan",
            RunnableKind::Console => "console",
        },
        "scope": match runnable.scope {
            RunnableScope::Class => "class",
            RunnableScope::Method => "method",
            RunnableScope::Test => "test",
            RunnableScope::Describe => "describe",
            RunnableScope::Arch => "arch",
            RunnableScope::Command => "command",
        },
        "label": runnable.label,
        "range": mapper.range(runnable.range),
        "filter": runnable.filter,
        "file": file.to_string_lossy(),
        "configFile": config.map(|config| config.to_string_lossy().into_owned()),
    })
}

fn lens_title(runnable: &Value) -> String {
    match runnable["scope"].as_str() {
        Some("class") => "Run tests in class",
        Some("describe") => "Run group",
        Some("command") => "Run command",
        _ => "Run test",
    }
    .to_string()
}

/// The PHPUnit configuration closest to a file, looking in its folder and then in each folder above
/// it up to the workspace folder.
fn config_file(file: &Path, folder: &Path) -> Option<PathBuf> {
    let mut current = file.parent();
    while let Some(directory) = current {
        for name in CONFIG_FILES {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        if directory == folder {
            break;
        }
        current = directory.parent();
    }
    None
}
