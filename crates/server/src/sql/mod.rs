//! SQL in the strings of PHP documents: the settings per document, the schema snapshots and the
//! DDL of the workspace's `.sql` files, the environments they make, and the analyses of the
//! strings of each open document, kept per version.

pub(crate) mod config;
mod features;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crossbeam_channel::Sender;
use lsc_server::paths::uri_to_path;
use lsp_types::{MessageType, ShowMessageParams, Uri};
use php_analysis::sql::{Embedded, embedded};
use php_index::Origin;
use php_syntax::TextRange;
use serde_json::Value;
use sql_embed::{Analysis, Dialect, Environment, Snapshot, Workspace, WorkspaceSchema};

pub(crate) use features::{FIX_ALL, Token, legend_modifiers, legend_types, merge_tokens};

use crate::documents::ParseDocument;
use crate::server::Server;
use crate::workspace::Internal;
use config::Resolved;

/// A string of a document read as SQL.
pub(crate) struct Injected {
    /// The string expression in the document.
    pub range: TextRange,
    pub analysis: Analysis,
}

impl Injected {
    /// Whether an offset is SQL of this string, and not a hole or a quote of it.
    pub fn holds(&self, offset: u32) -> bool {
        u32::from(self.range.start()) <= offset
            && offset <= u32::from(self.range.end())
            && self.analysis.to_sql(offset).is_some()
    }
}

struct CachedDocument {
    version: i32,
    generation: u64,
    items: Arc<Vec<Injected>>,
}

struct LoadedSnapshot {
    modified: Option<SystemTime>,
    snapshot: Option<Snapshot>,
}

/// What the server keeps for SQL.
#[derive(Default)]
pub(crate) struct SqlState {
    /// The `sql` setting the client gave at start or pushed since.
    pub setting: Option<Value>,
    /// The `.sql` files of the workspace folders with their text, once read.
    files: BTreeMap<PathBuf, String>,
    scan_started: bool,
    workspaces: HashMap<Dialect, Option<WorkspaceSchema>>,
    snapshots: HashMap<PathBuf, LoadedSnapshot>,
    environments: Vec<(sql_embed::Settings, Option<PathBuf>, Environment)>,
    documents: HashMap<Uri, CachedDocument>,
    /// Goes up whenever something every analysis was made with changes.
    generation: u64,
    /// The dialect each project's configuration names, by its root.
    project_dialects: HashMap<PathBuf, Option<Dialect>>,
    /// The problems with the settings and snapshots that were told already.
    told: HashSet<String>,
}

impl SqlState {
    pub fn new(setting: Option<Value>) -> SqlState {
        SqlState {
            setting,
            ..SqlState::default()
        }
    }

    /// Everything analyzed so far is out of date.
    pub fn changed(&mut self) {
        self.generation += 1;
        self.environments.clear();
        self.documents.clear();
    }

    /// The workspace folders changed: the `.sql` files are read again. Whether they had been read.
    pub fn forget_files(&mut self) -> bool {
        std::mem::take(&mut self.scan_started)
    }

    pub fn forget_document(&mut self, uri: &Uri) {
        self.documents.remove(uri);
    }

    /// The `.sql` files read in the background arrived.
    pub fn files_read(&mut self, files: Vec<(PathBuf, String)>) {
        self.files = files.into_iter().collect();
        self.workspaces.clear();
        self.changed();
    }

    pub fn set_project_dialect(&mut self, root: PathBuf, dialect: Option<Dialect>) -> bool {
        let changed = self.project_dialects.get(&root) != Some(&dialect);
        self.project_dialects.insert(root, dialect);
        if changed {
            self.changed();
        }
        changed
    }

    /// A watched `.sql` file or snapshot changed on disk; whether anything read from it changed.
    pub fn file_changed(&mut self, path: &Path, deleted: bool) -> bool {
        let mut changed = false;
        if self.snapshots.remove(path).is_some() {
            changed = true;
        }
        if self.scan_started && sql_embed::is_sql_file(path) {
            match (deleted, sql_embed::read_sql_file(path)) {
                (false, Some(text)) => {
                    self.files.insert(path.to_path_buf(), text);
                }
                _ => {
                    self.files.remove(path);
                }
            }
            self.workspaces.clear();
            changed = true;
        }
        if changed {
            self.changed();
        }
        changed
    }

    /// Whether a snapshot read before was written since, for a client that does not watch it.
    pub fn snapshots_changed(&mut self) -> bool {
        let stale: Vec<PathBuf> = self
            .snapshots
            .iter()
            .filter(|(path, loaded)| modified(path) != loaded.modified)
            .map(|(path, _)| path.clone())
            .collect();
        if stale.is_empty() {
            return false;
        }
        for path in stale {
            self.snapshots.remove(&path);
        }
        self.changed();
        true
    }

    /// The snapshots read so far, for the watcher the client is asked to keep.
    pub fn snapshot_paths(&self) -> Vec<PathBuf> {
        self.snapshots.keys().cloned().collect()
    }

    /// The `.sql` files of the workspace, for a search of references.
    pub fn sql_files(&self) -> &BTreeMap<PathBuf, String> {
        &self.files
    }

    fn workspace_schema(&mut self, dialect: Dialect) -> Option<WorkspaceSchema> {
        if let Some(found) = self.workspaces.get(&dialect) {
            return found.clone();
        }
        let mut workspace = Workspace::new(dialect);
        for (path, text) in &self.files {
            workspace.set_file(path.clone(), text);
        }
        let schema = workspace.schema();
        self.workspaces.insert(dialect, schema.clone());
        schema
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|metadata| metadata.modified()).ok()
}

/// The dialect a string is read in: the one a marker names, else the settings', else the one the
/// function it is passed to is for, else the one the project is configured for.
fn dialect_of(found: &Embedded, resolved: &Resolved, project: Option<Dialect>) -> Dialect {
    if found.marked_dialect {
        if let Some(dialect) = found.dialect {
            return dialect;
        }
    }
    if resolved.dialect_set {
        return resolved.settings.dialect;
    }
    match (found.dialect, project) {
        // A MySQL function reaches MariaDB as well.
        (Some(Dialect::Mysql), Some(Dialect::Mariadb)) => Dialect::Mariadb,
        (Some(dialect), _) | (None, Some(dialect)) => dialect,
        (None, None) => Dialect::Generic,
    }
}

impl Server {
    /// The `sql` setting for a document: its own answer, else what the client pushed.
    pub(crate) fn sql_resolved(&self, uri: &Uri) -> Resolved {
        let path = uri_to_path(uri);
        let setting = self
            .documents
            .get(uri)
            .and_then(|document| document.state.sql.clone())
            .or_else(|| self.sql.setting.clone());
        let base = path
            .as_deref()
            .and_then(|path| self.workspace.folder_of(path))
            .or_else(|| {
                self.workspace
                    .projects
                    .iter()
                    .find(|project| !project.is_nested())
                    .map(|project| project.root.clone())
            });
        config::resolve(setting.as_ref(), path.as_deref(), base.as_deref())
    }

    /// Tells a problem with the settings or a snapshot once.
    fn tell_once(&mut self, message: String, show: bool) {
        if !self.sql.told.insert(message.clone()) {
            return;
        }
        self.log(MessageType::WARNING, message.clone());
        if show {
            let _ = self
                .client
                .notify::<lsp_types::notification::ShowMessage>(ShowMessageParams {
                    typ: MessageType::WARNING,
                    message,
                });
        }
    }

    fn sql_snapshot(&mut self, path: Option<&Path>) -> Option<Snapshot> {
        let path = path?;
        if let Some(loaded) = self.sql.snapshots.get(path) {
            return loaded.snapshot.clone();
        }
        let snapshot = match Snapshot::load(path) {
            Ok(snapshot) => Some(snapshot),
            Err(message) => {
                self.tell_once(
                    format!("The SQL schema snapshot {} cannot be read: {message}", path.display()),
                    true,
                );
                None
            }
        };
        self.sql.snapshots.insert(
            path.to_path_buf(),
            LoadedSnapshot {
                modified: modified(path),
                snapshot: snapshot.clone(),
            },
        );
        self.watch_snapshots();
        snapshot
    }

    fn sql_environment(&mut self, resolved: &Resolved, dialect: Dialect) -> Environment {
        let mut settings = resolved.settings.clone();
        if settings.dialect != dialect {
            settings.dialect = dialect;
            settings.version = None;
            settings.sql_mode = None;
        }
        let found = self
            .sql
            .environments
            .iter()
            .find(|(known, schema, _)| *known == settings && *schema == resolved.schema)
            .map(|(_, _, environment)| environment.clone());
        if let Some(environment) = found {
            return environment;
        }
        let snapshot = self.sql_snapshot(resolved.schema.as_deref());
        let workspace = self.sql.workspace_schema(dialect);
        let environment = Environment::new(settings.clone(), snapshot, workspace);
        self.sql
            .environments
            .push((settings, resolved.schema.clone(), environment.clone()));
        environment
    }

    /// Reads the `.sql` files of the workspace folders in the background, once.
    pub(crate) fn scan_sql_files(&mut self) {
        if self.sql.scan_started {
            return;
        }
        self.sql.scan_started = true;
        let roots: Vec<PathBuf> = self
            .workspace
            .projects
            .iter()
            .filter(|project| !project.is_nested())
            .map(|project| project.root.clone())
            .collect();
        let sender: Sender<Internal> = self.internal_sender.clone();
        std::thread::spawn(move || {
            let files = sql_embed::sql_files(&roots)
                .into_iter()
                .filter_map(|path| {
                    let text = sql_embed::read_sql_file(&path)?;
                    Some((path, text))
                })
                .collect();
            let _ = sender.send(Internal::SqlFiles(files));
        });
    }

    /// Works out in the background which database a project's configuration names.
    pub(crate) fn find_project_dialect(&self, root: &Path) {
        let Some(project) = self.workspace.projects.iter().find(|project| project.root == root) else {
            return;
        };
        let frameworks = project.index.frameworks();
        let files: Vec<PathBuf> = if frameworks.raxos {
            project
                .index
                .files()
                .filter(|file| file.origin == Origin::Project)
                .map(|file| file.path.clone())
                .collect()
        } else {
            Vec::new()
        };
        let root = root.to_path_buf();
        let sender = self.internal_sender.clone();
        std::thread::spawn(move || {
            let dialect = php_analysis::sql::project_dialect(&root, frameworks, &files);
            let _ = sender.send(Internal::SqlDialect { root, dialect });
        });
    }

    /// The strings of an open PHP document that hold SQL, analyzed, for its current version.
    pub(crate) fn sql_injected(&mut self, uri: &Uri) -> Option<Arc<Vec<Injected>>> {
        let document = self.documents.get(uri)?;
        if document.state.blade || document.state.twig || document.state.yaml {
            return None;
        }
        let version = document.version;
        let generation = self.sql.generation;
        if let Some(cached) = self.sql.documents.get(uri) {
            if cached.version == version && cached.generation == generation {
                return Some(cached.items.clone());
            }
        }
        let resolved = self.sql_resolved(uri);
        for problem in &resolved.problems {
            self.tell_once(format!("The sql setting: {problem}"), false);
        }
        if !resolved.enabled {
            return Some(Arc::new(Vec::new()));
        }
        self.scan_sql_files();
        self.sync_symbols(uri);
        let path = uri_to_path(uri);
        let _document = php_analysis::document::enter(path.as_deref());
        let project_dialect = path.as_deref().and_then(|path| {
            let root = &self.workspace.project_for(path).root;
            self.sql.project_dialects.get(root).copied().flatten()
        });
        let found = {
            let document = self.documents.get_mut(uri)?;
            let root = document.parse().syntax();
            let project = match &path {
                Some(path) => self.workspace.project_for(path),
                None => &self.workspace.loose,
            };
            embedded(&project.index, &root, resolved.detection)
        };
        let mut items = Vec::with_capacity(found.len());
        for string in found {
            let dialect = dialect_of(&string, &resolved, project_dialect);
            let environment = self.sql_environment(&resolved, dialect);
            items.push(Injected {
                range: string.range,
                analysis: Analysis::new(&environment, &string.fragment),
            });
        }
        let items = Arc::new(items);
        self.sql.documents.insert(
            uri.clone(),
            CachedDocument {
                version,
                generation,
                items: items.clone(),
            },
        );
        Some(items)
    }

    /// The string of SQL at an offset of a document, when the offset is in its SQL.
    pub(crate) fn sql_at(&mut self, uri: &Uri, offset: u32) -> Option<(Arc<Vec<Injected>>, usize)> {
        let items = self.sql_injected(uri)?;
        let position = items.iter().position(|item| item.holds(offset))?;
        Some((items, position))
    }
}
