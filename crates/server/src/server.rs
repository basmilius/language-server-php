use std::collections::HashMap;
use std::error::Error;
use std::path::{Path, PathBuf};

use crossbeam_channel::{Receiver, Sender, select};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeConfiguration, DidChangeTextDocument, DidChangeWatchedFiles, DidChangeWorkspaceFolders,
    DidCloseTextDocument, DidOpenTextDocument, LogMessage, Notification as _, Progress as ProgressNotification,
    PublishDiagnostics,
};
use lsp_types::request::{
    CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare, InlayHintRefreshRequest,
    InlayHintRequest, SemanticTokensFullRequest, SemanticTokensRangeRequest, SemanticTokensRefresh,
    SignatureHelpRequest, TypeHierarchyPrepare, TypeHierarchySubtypes, TypeHierarchySupertypes,
};
use lsp_types::request::{CodeActionRequest, CodeActionResolveRequest, Formatting, OnTypeFormatting, RangeFormatting};
use lsp_types::request::{
    Completion, DocumentDiagnosticRequest, DocumentHighlightRequest, DocumentSymbolRequest, FoldingRangeRequest,
    GotoDefinition, GotoImplementation, GotoTypeDefinition, HoverRequest, PrepareRenameRequest, References,
    RegisterCapability, Rename, Request as _, ResolveCompletionItem, SelectionRangeRequest, WorkDoneProgressCreate,
    WorkspaceConfiguration, WorkspaceDiagnosticRefresh, WorkspaceSymbolRequest,
};
use lsp_types::{
    CompletionOptions, ConfigurationItem, ConfigurationParams, DiagnosticOptions, DiagnosticServerCapabilities,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentDiagnosticParams, DocumentDiagnosticReport, DocumentDiagnosticReportResult,
    DocumentSymbolParams, DocumentSymbolResponse, FileChangeType, FileSystemWatcher, FoldingRangeParams,
    FoldingRangeProviderCapability, FullDocumentDiagnosticReport, GlobPattern, HoverProviderCapability,
    ImplementationProviderCapability, InitializeParams, InitializeResult, LogMessageParams, MessageType, OneOf,
    ProgressParams, ProgressParamsValue, PublishDiagnosticsParams, Registration, RegistrationParams,
    RelatedFullDocumentDiagnosticReport, SelectionRangeParams, SelectionRangeProviderCapability, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TypeDefinitionProviderCapability, Uri, WorkDoneProgress, WorkDoneProgressBegin, WorkDoneProgressCreateParams,
    WorkDoneProgressEnd, WorkDoneProgressOptions, WorkDoneProgressReport, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities,
};
use php_analysis::inspections::{Externals, InspectionEnv, inspect};
use php_analysis::{PositionEncoding, diagnostics, document_symbols, folding_ranges, selection_ranges};
use php_index::StubFile;
use php_index::indexer::{IndexEvent, index_file};
use php_syntax::PhpVersion;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::{SECTION, Settings};
use crate::convert::{self, Mapper};
use crate::documents::Documents;
use crate::paths::uri_to_path;
use crate::workspace::{FolderChange, Internal, Workspace};

/// How many files read from the cache file stay in memory, the most recently used ones.
/// What the server asks the client to watch: the PHP files and Composer's, and the files of the
/// frameworks that are not PHP and that names in strings are read from.
const WATCHED_FILES: [&str; 14] = [
    "**/*.php",
    "**/composer.json",
    "**/vendor/composer/installed.json",
    // A client that lets `vendor` through only for a glob that names it would drop this under `**/*.php`.
    "**/vendor/composer/autoload_classmap.php",
    "**/.env",
    "**/.env.*",
    "**/lang/**",
    "**/translations/**",
    "**/templates/**",
    "**/config/**/*.yaml",
    "**/config/**/*.yml",
    "**/config/**/*.xml",
    "**/database/schema/*.sql",
    "**/resources/{js,ts}/{Pages,pages}/**",
];

const KEEP_LOADED_FILES: usize = 1500;
const TRIM_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);

pub(crate) type BoxError = Box<dyn Error + Send + Sync>;

/// Runs the server on a connection until the client shuts it down.
pub fn run(connection: Connection) -> Result<(), BoxError> {
    let (id, raw) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(raw.clone())?;
    let mut server = Server::new(&connection, &params);
    // `lsp-types` has no field for snippet edits yet.
    server.snippet_edits = raw
        .pointer("/capabilities/workspace/workspaceEdit/snippetEditSupport")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let result = InitializeResult {
        capabilities: server.capabilities(),
        server_info: Some(ServerInfo {
            name: "php-language-server".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };
    // `initialize_finish` waits for and consumes the `initialized` notification.
    let mut result = serde_json::to_value(result)?;
    // `lsp-types` has no field for the type hierarchy provider yet.
    result["capabilities"]["typeHierarchyProvider"] = Value::Bool(true);
    connection.initialize_finish(id, result)?;
    server.initialized()?;
    server.main_loop(&connection.receiver)
}

/// The indexing work in flight, as one `$/progress` the client can show.
#[derive(Default)]
struct Progress {
    supported: bool,
    counter: u32,
    token: Option<String>,
    /// The client answered `window/workDoneProgress/create` and the report has begun.
    begun: bool,
    create_request: Option<RequestId>,
    running_jobs: usize,
    total: usize,
    done: usize,
    last_percent: u32,
}

pub(crate) struct Server<'a> {
    pub(crate) connection: &'a Connection,
    pub(crate) documents: Documents,
    pub(crate) encoding: PositionEncoding,
    pub(crate) settings: Settings,
    /// The client pulls diagnostics, so the server does not push them.
    pull_diagnostics: bool,
    hierarchical_symbols: bool,
    configuration_support: bool,
    diagnostic_refresh_support: bool,
    watch_support: bool,
    semantic_refresh_support: bool,
    inlay_refresh_support: bool,
    /// The client takes `documentChanges` in a workspace edit.
    pub(crate) document_changes: bool,
    /// The client can rename files as part of a workspace edit.
    pub(crate) rename_files: bool,
    /// The client can make files as part of a workspace edit.
    pub(crate) create_files: bool,
    /// The client can ask for the edit of a code action after the list is shown.
    pub(crate) code_action_resolve: bool,
    /// The client takes snippet text edits in a workspace edit.
    pub(crate) snippet_edits: bool,
    /// Documents whose diagnostics are out of date, published once the queue of messages is empty.
    dirty: Vec<Uri>,
    /// Questions asked of the client: which document each `workspace/configuration` answer is for.
    pending_configuration: HashMap<RequestId, Uri>,
    next_request_id: i32,
    last_trim: std::time::Instant,
    pub(crate) workspace: Workspace,
    initial_folders: Vec<PathBuf>,
    internal_sender: Sender<Internal>,
    internal_receiver: Receiver<Internal>,
    progress: Progress,
    /// What the places that render a template give it, by template, until a file other than the
    /// template itself changes.
    pub(crate) given_cache: HashMap<PathBuf, Vec<(String, php_index::Type)>>,
}

/// Whether Composer's autoload maps point at a file that exists for the class, which the index
/// reads when something asks for it.
fn composer_loads(project: &php_index::Project, class: &str) -> bool {
    project.composer.as_ref().is_some_and(|composer| {
        composer
            .class_candidates(class)
            .iter()
            .any(|candidate| candidate.is_file())
    })
}

fn folders_of(params: &InitializeParams) -> Vec<PathBuf> {
    let mut folders: Vec<PathBuf> = params
        .workspace_folders
        .iter()
        .flatten()
        .filter_map(|folder| uri_to_path(&folder.uri))
        .collect();
    #[allow(deprecated)]
    if folders.is_empty() {
        if let Some(root) = params.root_uri.as_ref().and_then(uri_to_path) {
            folders.push(root);
        }
    }
    folders
}

impl<'a> Server<'a> {
    fn new(connection: &'a Connection, params: &InitializeParams) -> Server<'a> {
        let capabilities = &params.capabilities;
        let settings = params
            .initialization_options
            .as_ref()
            .map(Settings::from_value)
            .unwrap_or_default();
        let text_document = capabilities.text_document.as_ref();
        let workspace_edit = capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref());
        let (internal_sender, internal_receiver) = crossbeam_channel::unbounded();
        let workspace = Workspace::new(
            settings.storage_path.clone(),
            settings.stubs_path.clone(),
            settings.php_version.unwrap_or(PhpVersion::DEFAULT),
        );
        Server {
            connection,
            documents: Documents::default(),
            encoding: convert::choose_encoding(
                capabilities
                    .general
                    .as_ref()
                    .and_then(|general| general.position_encodings.as_deref()),
            ),
            settings,
            pull_diagnostics: text_document.is_some_and(|text_document| text_document.diagnostic.is_some()),
            hierarchical_symbols: text_document
                .and_then(|text_document| text_document.document_symbol.as_ref())
                .and_then(|symbol| symbol.hierarchical_document_symbol_support)
                .unwrap_or(false),
            configuration_support: capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.configuration)
                .unwrap_or(false),
            diagnostic_refresh_support: capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.diagnostic.as_ref())
                .and_then(|diagnostic| diagnostic.refresh_support)
                .unwrap_or(false),
            watch_support: capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.did_change_watched_files.as_ref())
                .and_then(|watched| watched.dynamic_registration)
                .unwrap_or(false),
            semantic_refresh_support: capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.semantic_tokens.as_ref())
                .and_then(|tokens| tokens.refresh_support)
                .unwrap_or(false),
            inlay_refresh_support: capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.inlay_hint.as_ref())
                .and_then(|hints| hints.refresh_support)
                .unwrap_or(false),
            document_changes: workspace_edit.and_then(|edit| edit.document_changes).unwrap_or(false),
            rename_files: workspace_edit
                .and_then(|edit| edit.resource_operations.as_ref())
                .is_some_and(|operations| operations.contains(&lsp_types::ResourceOperationKind::Rename)),
            create_files: workspace_edit
                .and_then(|edit| edit.resource_operations.as_ref())
                .is_some_and(|operations| operations.contains(&lsp_types::ResourceOperationKind::Create)),
            code_action_resolve: text_document
                .and_then(|text_document| text_document.code_action.as_ref())
                .and_then(|actions| actions.resolve_support.as_ref())
                .is_some_and(|support| support.properties.iter().any(|property| property == "edit")),
            snippet_edits: false,
            dirty: Vec::new(),
            pending_configuration: HashMap::new(),
            next_request_id: 0,
            last_trim: std::time::Instant::now(),
            workspace,
            initial_folders: folders_of(params),
            internal_sender,
            internal_receiver,
            progress: Progress {
                supported: capabilities
                    .window
                    .as_ref()
                    .and_then(|window| window.work_done_progress)
                    .unwrap_or(false),
                ..Progress::default()
            },
            given_cache: HashMap::new(),
        }
    }

    fn capabilities(&self) -> ServerCapabilities {
        ServerCapabilities {
            position_encoding: Some(convert::encoding_kind(self.encoding)),
            text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                ..TextDocumentSyncOptions::default()
            })),
            document_symbol_provider: Some(OneOf::Left(true)),
            folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
            selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            definition_provider: Some(OneOf::Left(true)),
            type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
            implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
            workspace_symbol_provider: Some(OneOf::Left(true)),
            references_provider: Some(OneOf::Left(true)),
            inlay_hint_provider: Some(OneOf::Left(true)),
            semantic_tokens_provider: Some(lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(
                lsp_types::SemanticTokensOptions {
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                    legend: crate::insight::semantic_legend(),
                    range: Some(true),
                    full: Some(lsp_types::SemanticTokensFullOptions::Bool(true)),
                },
            )),
            call_hierarchy_provider: Some(lsp_types::CallHierarchyServerCapability::Simple(true)),
            signature_help_provider: Some(lsp_types::SignatureHelpOptions {
                trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                retrigger_characters: Some(vec![",".to_string()]),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            }),
            rename_provider: Some(OneOf::Right(lsp_types::RenameOptions {
                prepare_provider: Some(true),
                work_done_progress_options: WorkDoneProgressOptions::default(),
            })),
            document_highlight_provider: Some(OneOf::Left(true)),
            code_action_provider: Some(lsp_types::CodeActionProviderCapability::Options(
                lsp_types::CodeActionOptions {
                    code_action_kinds: Some(vec![
                        lsp_types::CodeActionKind::QUICKFIX,
                        lsp_types::CodeActionKind::REFACTOR,
                        lsp_types::CodeActionKind::REFACTOR_REWRITE,
                        lsp_types::CodeActionKind::new("refactor.extract"),
                        lsp_types::CodeActionKind::new("refactor.inline"),
                        lsp_types::CodeActionKind::new("refactor.move"),
                        lsp_types::CodeActionKind::SOURCE,
                        lsp_types::CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                    ]),
                    resolve_provider: Some(true),
                    work_done_progress_options: WorkDoneProgressOptions::default(),
                },
            )),
            execute_command_provider: Some(lsp_types::ExecuteCommandOptions {
                commands: vec![crate::refactors::RENAME_COMMAND.to_string()],
                work_done_progress_options: WorkDoneProgressOptions::default(),
            }),
            document_formatting_provider: Some(OneOf::Left(true)),
            document_range_formatting_provider: Some(OneOf::Left(true)),
            document_on_type_formatting_provider: Some(lsp_types::DocumentOnTypeFormattingOptions {
                first_trigger_character: "\n".to_string(),
                more_trigger_character: Some(vec!["}".to_string(), ";".to_string()]),
            }),
            code_lens_provider: Some(lsp_types::CodeLensOptions {
                resolve_provider: Some(false),
            }),
            completion_provider: Some(CompletionOptions {
                resolve_provider: Some(true),
                trigger_characters: Some(["$", ">", ":", "\\", "#", "["].map(String::from).to_vec()),
                all_commit_characters: None,
                work_done_progress_options: WorkDoneProgressOptions::default(),
                completion_item: None,
            }),
            workspace: Some(WorkspaceServerCapabilities {
                workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                    supported: Some(true),
                    change_notifications: Some(OneOf::Left(true)),
                }),
                file_operations: Some(lsp_types::WorkspaceFileOperationsServerCapabilities {
                    will_rename: Some(lsp_types::FileOperationRegistrationOptions {
                        filters: vec![
                            lsp_types::FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: lsp_types::FileOperationPattern {
                                    glob: "**/*.php".to_string(),
                                    matches: Some(lsp_types::FileOperationPatternKind::File),
                                    options: None,
                                },
                            },
                            lsp_types::FileOperationFilter {
                                scheme: Some("file".to_string()),
                                pattern: lsp_types::FileOperationPattern {
                                    glob: "**".to_string(),
                                    matches: Some(lsp_types::FileOperationPatternKind::Folder),
                                    options: None,
                                },
                            },
                        ],
                    }),
                    ..Default::default()
                }),
            }),
            diagnostic_provider: self.pull_diagnostics.then(|| {
                DiagnosticServerCapabilities::Options(DiagnosticOptions {
                    identifier: Some("php".to_string()),
                    inter_file_dependencies: false,
                    workspace_diagnostics: false,
                    work_done_progress_options: Default::default(),
                })
            }),
            ..ServerCapabilities::default()
        }
    }

    fn main_loop(&mut self, receiver: &Receiver<Message>) -> Result<(), BoxError> {
        let internal = self.internal_receiver.clone();
        loop {
            select! {
                recv(receiver) -> message => {
                    let Ok(message) = message else {
                        return Ok(());
                    };
                    if self.handle(message)? {
                        return Ok(());
                    }
                    // Typing sends a change per keystroke: answer them all before spending time on diagnostics.
                    while let Ok(message) = receiver.try_recv() {
                        if self.handle(message)? {
                            return Ok(());
                        }
                    }
                    self.publish_dirty()?;
                    self.trim_indexes();
                }
                recv(internal) -> event => {
                    if let Ok(event) = event {
                        self.internal(event)?;
                    }
                }
            }
        }
    }

    /// Lets go of the declarations that were read from the cache file and have not been used for a
    /// while, at most every few seconds.
    fn trim_indexes(&mut self) {
        if self.last_trim.elapsed() < TRIM_INTERVAL {
            return;
        }
        self.last_trim = std::time::Instant::now();
        self.workspace.trim_indexes(KEEP_LOADED_FILES);
    }

    /// Handles one message and says whether the server is done.
    fn handle(&mut self, message: Message) -> Result<bool, BoxError> {
        match message {
            Message::Request(request) => {
                if self.connection.handle_shutdown(&request)? {
                    return Ok(true);
                }
                self.request(request)?;
            }
            Message::Notification(notification) => self.notification(notification)?,
            Message::Response(response) => self.response(response)?,
        }
        Ok(false)
    }

    pub(crate) fn send(&self, message: impl Into<Message>) -> Result<(), BoxError> {
        self.connection.sender.send(message.into())?;
        Ok(())
    }

    fn request(&mut self, request: Request) -> Result<(), BoxError> {
        let id = request.id.clone();
        let document = request
            .params
            .pointer("/textDocument/uri")
            .or_else(|| request.params.pointer("/uri"))
            .and_then(Value::as_str)
            .and_then(|uri| uri.parse::<Uri>().ok())
            .and_then(|uri| uri_to_path(&uri));
        let _document = php_analysis::document::enter(document.as_deref());
        let response = match request.method.as_str() {
            DocumentSymbolRequest::METHOD => self.answer(id, request.params, Self::document_symbols),
            FoldingRangeRequest::METHOD => self.answer(id, request.params, Self::folding_ranges),
            SelectionRangeRequest::METHOD => self.answer(id, request.params, Self::selection_ranges),
            DocumentDiagnosticRequest::METHOD => self.answer(id, request.params, Self::pull_diagnostics_for),
            HoverRequest::METHOD => self.answer(id, request.params, Self::hover),
            GotoDefinition::METHOD => self.answer(id, request.params, Self::definition),
            GotoTypeDefinition::METHOD => self.answer(id, request.params, Self::type_definition),
            GotoImplementation::METHOD => self.answer(id, request.params, Self::implementation),
            WorkspaceSymbolRequest::METHOD => self.answer(id, request.params, Self::workspace_symbols),
            References::METHOD => self.answer(id, request.params, Self::references),
            SignatureHelpRequest::METHOD => self.answer(id, request.params, Self::signature_help),
            InlayHintRequest::METHOD => self.answer(id, request.params, Self::inlay_hints),
            SemanticTokensFullRequest::METHOD => self.answer(id, request.params, Self::semantic_tokens_full),
            SemanticTokensRangeRequest::METHOD => self.answer(id, request.params, Self::semantic_tokens_range),
            CallHierarchyPrepare::METHOD => self.answer(id, request.params, Self::prepare_call_hierarchy),
            CallHierarchyIncomingCalls::METHOD => self.answer(id, request.params, Self::incoming_calls),
            CallHierarchyOutgoingCalls::METHOD => self.answer(id, request.params, Self::outgoing_calls),
            TypeHierarchyPrepare::METHOD => self.answer(id, request.params, Self::prepare_type_hierarchy),
            TypeHierarchySupertypes::METHOD => self.answer(id, request.params, Self::type_hierarchy_supertypes),
            TypeHierarchySubtypes::METHOD => self.answer(id, request.params, Self::type_hierarchy_subtypes),
            PrepareRenameRequest::METHOD => self.answer_checked(id, request.params, Self::prepare_rename),
            Rename::METHOD => self.answer_checked(id, request.params, Self::rename),
            DocumentHighlightRequest::METHOD => self.answer(id, request.params, Self::document_highlight),
            CodeActionRequest::METHOD => self.answer(id, request.params, Self::code_action),
            CodeActionResolveRequest::METHOD => self.answer_checked(id, request.params, Self::resolve_code_action),
            lsp_types::request::ExecuteCommand::METHOD => Response::new_ok(id, Value::Null),
            lsp_types::request::WillRenameFiles::METHOD => {
                self.answer_checked(id, request.params, Self::will_rename_files)
            }
            Formatting::METHOD => self.answer(id, request.params, Self::formatting),
            RangeFormatting::METHOD => self.answer(id, request.params, Self::range_formatting),
            OnTypeFormatting::METHOD => self.answer(id, request.params, Self::on_type_formatting),
            Completion::METHOD => self.answer(id, request.params, Self::completion),
            lsp_types::request::CodeLensRequest::METHOD => self.answer(id, request.params, Self::code_lens),
            crate::runnables::RUNNABLES_METHOD => self.answer(id, request.params, Self::runnables),
            ResolveCompletionItem::METHOD => self.answer(id, request.params, Self::resolve_completion),
            method => Response::new_err(
                id,
                ErrorCode::MethodNotFound as i32,
                format!("unsupported request {method}"),
            ),
        };
        self.send(response)
    }

    /// Decodes the parameters, runs a handler and wraps what it returns in a response.
    fn answer<P, R>(&mut self, id: RequestId, params: Value, handler: fn(&mut Self, P) -> Option<R>) -> Response
    where
        P: DeserializeOwned,
        R: serde::Serialize,
    {
        match serde_json::from_value::<P>(params) {
            Ok(params) => {
                let result = handler(self, params);
                Response::new_ok(id, result)
            }
            Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
        }
    }

    /// Like [`Self::answer`] for a handler that can refuse with a message the client shows.
    fn answer_checked<P, R>(
        &mut self,
        id: RequestId,
        params: Value,
        handler: fn(&mut Self, P) -> Result<Option<R>, String>,
    ) -> Response
    where
        P: DeserializeOwned,
        R: serde::Serialize,
    {
        match serde_json::from_value::<P>(params) {
            Ok(params) => match handler(self, params) {
                Ok(result) => Response::new_ok(id, result),
                Err(message) => Response::new_err(id, ErrorCode::RequestFailed as i32, message),
            },
            Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
        }
    }

    fn document_symbols(&mut self, params: DocumentSymbolParams) -> Option<DocumentSymbolResponse> {
        let uri = params.text_document.uri;
        let hierarchical = self.hierarchical_symbols;
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let symbols = document_symbols(&document.parse().syntax());
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        if hierarchical {
            return Some(DocumentSymbolResponse::Nested(convert::hierarchical_symbols(
                &mapper, &symbols,
            )));
        }
        let mut flat = Vec::new();
        convert::flat_symbols(&mapper, &uri, &symbols, None, &mut flat);
        Some(DocumentSymbolResponse::Flat(flat))
    }

    fn folding_ranges(&mut self, params: FoldingRangeParams) -> Option<Vec<lsp_types::FoldingRange>> {
        let document = self.documents.get_mut(&params.text_document.uri)?;
        let root = document.parse().syntax();
        let folds = folding_ranges(&root, &document.text, &document.index);
        Some(folds.iter().map(convert::folding_range).collect())
    }

    fn selection_ranges(&mut self, params: SelectionRangeParams) -> Option<Vec<lsp_types::SelectionRange>> {
        let encoding = self.encoding;
        let document = self.documents.get_mut(&params.text_document.uri)?;
        let root = document.parse().syntax();
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        Some(
            params
                .positions
                .iter()
                .map(|position| convert::selection_chain(&mapper, &selection_ranges(&root, mapper.offset(*position))))
                .collect(),
        )
    }

    fn pull_diagnostics_for(&mut self, params: DocumentDiagnosticParams) -> Option<DocumentDiagnosticReportResult> {
        let items = self.diagnostics_of(&params.text_document.uri).unwrap_or_default();
        Some(DocumentDiagnosticReportResult::Report(DocumentDiagnosticReport::Full(
            RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport { result_id: None, items },
            },
        )))
    }

    /// The level a document is read at: its project's, when `composer.json` names one, else the
    /// client's answer for the document, else the default, else the newest.
    fn level_of(&self, uri: &Uri, document_level: Option<PhpVersion>) -> PhpVersion {
        if let Some(path) = uri_to_path(uri) {
            let project = self.workspace.project_for(&path);
            if project.level_from_composer {
                return project.level;
            }
        }
        document_level
            .or(self.settings.php_version)
            .unwrap_or(PhpVersion::DEFAULT)
    }

    fn diagnostics_of(&mut self, uri: &Uri) -> Option<Vec<lsp_types::Diagnostic>> {
        let document = self.documents.get(uri)?;
        if document.blade || document.twig || document.yaml {
            return self.template_diagnostics(uri);
        }
        let _document = php_analysis::document::enter(uri_to_path(uri).as_deref());
        let level = self.level_of(uri, self.documents.get(uri)?.level);
        self.inspect_document(uri, |env, mapper, parse| {
            let mut found = diagnostics(parse, level);
            found.extend(inspect(env).into_iter().map(|finding| finding.diagnostic));
            found.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
            found.iter().map(|found| mapper.diagnostic(found)).collect()
        })
    }

    /// What is certainly wrong in an open Blade or Twig template.
    fn template_diagnostics(&mut self, uri: &Uri) -> Option<Vec<lsp_types::Diagnostic>> {
        let path = uri_to_path(uri);
        let given = self.template_given(uri);
        // A package's template reads keys the package's users may set; only the project's own are judged.
        let ready = path.as_ref().is_some_and(|path| {
            self.workspace.is_ready(path)
                && self.workspace.project_for(path).origin_of(path) == php_index::Origin::Project
        });
        let encoding = self.encoding;
        let document = self.documents.get(uri)?;
        let settings = document
            .inspections
            .clone()
            .or_else(|| self.settings.inspections.clone())
            .unwrap_or_default();
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let found = if document.yaml {
            match &path {
                Some(path) if php_analysis::yaml::is_config(&project.index, path) => {
                    let loadable = |class: &str| composer_loads(project, class);
                    php_analysis::yaml::diagnostics(&project.index, path, &document.text, &settings, ready, &loadable)
                }
                _ => Vec::new(),
            }
        } else if document.twig {
            php_analysis::twig::diagnostics(&project.index, path.as_deref(), &document.text, &settings, ready)
        } else {
            php_analysis::blade::diagnostics(
                &project.index,
                path.as_deref(),
                &document.text,
                &given,
                &settings,
                ready,
            )
        };
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        Some(found.iter().map(|found| mapper.diagnostic(found)).collect())
    }

    /// Runs something over an open document with everything an inspection reads: the tree, the
    /// index of its project, what the client chose to switch on and which names are held back by
    /// extensions the project does not require.
    pub(crate) fn inspect_document<R>(
        &mut self,
        uri: &Uri,
        run: impl FnOnce(&InspectionEnv<'_>, &Mapper<'_>, &php_syntax::Parse) -> R,
    ) -> Option<R> {
        self.inspect_document_in(uri, |env, mapper, parse, _| run(env, mapper, parse))
    }

    /// Like [`Self::inspect_document`], and the project the document belongs to as well.
    pub(crate) fn inspect_document_in<R>(
        &mut self,
        uri: &Uri,
        run: impl FnOnce(&InspectionEnv<'_>, &Mapper<'_>, &php_syntax::Parse, &php_index::Project) -> R,
    ) -> Option<R> {
        self.sync_symbols(uri);
        let encoding = self.encoding;
        let path = uri_to_path(uri);
        let ready = path.as_ref().is_some_and(|path| self.workspace.is_ready(path));
        let document = self.documents.get_mut(uri)?;
        document.parse();
        let parse = document.cached()?;
        let root = parse.syntax();
        let settings = document
            .inspections
            .clone()
            .or_else(|| self.settings.inspections.clone())
            .unwrap_or_default();
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let names = &self.workspace.stub_names;
        let has_class = |name: &str| names.has_class(name) || composer_loads(project, name);
        let has_function = |name: &str| names.has_function(name);
        let has_constant = |name: &str| names.has_constant(name);
        let externals = Externals {
            class: &has_class,
            function: &has_function,
            constant: &has_constant,
        };
        let env = InspectionEnv {
            index: &project.index,
            text: &document.text,
            root: &root,
            settings: &settings,
            ready,
            externals: &externals,
        };
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        Some(run(&env, &mapper, parse, project))
    }

    fn notification(&mut self, notification: Notification) -> Result<(), BoxError> {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let params: DidOpenTextDocumentParams = serde_json::from_value(notification.params)?;
                let item = params.text_document;
                self.documents.open(item.uri.clone(), item.version, item.text);
                let is_blade = item.language_id == "blade" || item.uri.as_str().ends_with(".blade.php");
                let is_twig = item.language_id == "twig" || item.uri.as_str().ends_with(".twig");
                let is_yaml = item.language_id == "yaml"
                    || item.uri.as_str().ends_with(".yaml")
                    || item.uri.as_str().ends_with(".yml");
                if let Some(document) = self.documents.get_mut(&item.uri) {
                    document.blade = is_blade;
                    document.twig = is_twig && !is_blade;
                    document.yaml = is_yaml && !is_twig && !is_blade;
                }
                self.sync_symbols(&item.uri);
                self.request_configuration(&item.uri)?;
                self.mark_dirty(item.uri);
            }
            DidChangeTextDocument::METHOD => {
                let params: DidChangeTextDocumentParams = serde_json::from_value(notification.params)?;
                let uri = params.text_document.uri;
                if let Some(document) = self.documents.get_mut(&uri) {
                    document.apply_changes(params.text_document.version, &params.content_changes, self.encoding);
                    if let Some(path) = uri_to_path(&uri) {
                        self.given_cache.retain(|template, _| *template == path);
                    }
                    self.mark_dirty(uri);
                }
            }
            DidCloseTextDocument::METHOD => {
                let params: DidCloseTextDocumentParams = serde_json::from_value(notification.params)?;
                let uri = params.text_document.uri;
                self.documents.close(&uri);
                self.given_cache.clear();
                self.dirty.retain(|dirty| *dirty != uri);
                self.reindex_from_disk(&uri);
                if !self.pull_diagnostics {
                    self.publish(uri, None, Vec::new())?;
                }
            }
            DidChangeConfiguration::METHOD => {
                let params: DidChangeConfigurationParams = serde_json::from_value(notification.params)?;
                self.configuration_changed(&params.settings)?;
            }
            DidChangeWatchedFiles::METHOD => {
                let params: DidChangeWatchedFilesParams = serde_json::from_value(notification.params)?;
                self.watched_files_changed(params)?;
            }
            DidChangeWorkspaceFolders::METHOD => {
                let params: DidChangeWorkspaceFoldersParams = serde_json::from_value(notification.params)?;
                for folder in params.event.removed {
                    if let Some(path) = uri_to_path(&folder.uri) {
                        self.workspace.remove_folder(&path);
                    }
                }
                for folder in params.event.added {
                    if let Some(path) = uri_to_path(&folder.uri) {
                        self.open_folder(&path);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The client is ready: ask it to watch files, open the folders and start reading the stubs.
    fn initialized(&mut self) -> Result<(), BoxError> {
        if self.watch_support {
            let watchers = WATCHED_FILES
                .map(|pattern| FileSystemWatcher {
                    glob_pattern: GlobPattern::String(pattern.to_string()),
                    kind: None,
                })
                .to_vec();
            let options = DidChangeWatchedFilesRegistrationOptions { watchers };
            self.next_request_id += 1;
            let id = RequestId::from(self.next_request_id);
            self.send(Request::new(
                id,
                RegisterCapability::METHOD.to_string(),
                RegistrationParams {
                    registrations: vec![Registration {
                        id: "php-watched-files".to_string(),
                        method: DidChangeWatchedFiles::METHOD.to_string(),
                        register_options: Some(serde_json::to_value(options)?),
                    }],
                },
            ))?;
        }
        self.workspace.start_stubs_job(&self.internal_sender);
        if self.workspace.storage.is_some() || self.workspace.stubs_override.is_some() {
            self.job_started();
        }
        for folder in std::mem::take(&mut self.initial_folders) {
            self.open_folder(&folder);
        }
        Ok(())
    }

    fn open_folder(&mut self, root: &Path) {
        let change = self.workspace.add_folder(root);
        self.apply_folder_change(change);
    }

    fn apply_folder_change(&mut self, change: FolderChange) {
        for root in change.start {
            self.workspace.start_project_job(&root, &self.internal_sender);
            self.job_started();
        }
        for root in change.restart {
            self.restart_project(&root);
        }
    }

    // Background indexing --------------------------------------------------------------------

    fn job_started(&mut self) {
        self.progress.running_jobs += 1;
        if self.progress.supported && self.progress.token.is_none() {
            self.progress.counter += 1;
            let token = format!("php-language-server/indexing/{}", self.progress.counter);
            self.next_request_id += 1;
            let id = RequestId::from(self.next_request_id);
            self.progress.token = Some(token.clone());
            self.progress.begun = false;
            self.progress.create_request = Some(id.clone());
            let _ = self.send(Request::new(
                id,
                WorkDoneProgressCreate::METHOD.to_string(),
                WorkDoneProgressCreateParams {
                    token: lsp_types::NumberOrString::String(token),
                },
            ));
        }
    }

    fn progress_percent(&self) -> u32 {
        if self.progress.total == 0 {
            return 0;
        }
        ((self.progress.done * 100) / self.progress.total).min(100) as u32
    }

    fn send_progress(&self, value: WorkDoneProgress) {
        let Some(token) = &self.progress.token else {
            return;
        };
        let _ = self.send(Notification::new(
            ProgressNotification::METHOD.to_string(),
            ProgressParams {
                token: lsp_types::NumberOrString::String(token.clone()),
                value: ProgressParamsValue::WorkDone(value),
            },
        ));
    }

    fn job_progress(&mut self, files: usize, discovered: usize) {
        self.progress.total += discovered;
        self.progress.done += files;
        let percent = self.progress_percent();
        if self.progress.begun && percent != self.progress.last_percent {
            self.progress.last_percent = percent;
            self.send_progress(WorkDoneProgress::Report(WorkDoneProgressReport {
                cancellable: Some(false),
                message: Some(format!("{} of {} files", self.progress.done, self.progress.total)),
                percentage: Some(percent),
            }));
        }
    }

    fn job_finished(&mut self) {
        self.progress.running_jobs = self.progress.running_jobs.saturating_sub(1);
        // While the client has not answered the request to create the progress, its answer ends it.
        if self.progress.running_jobs > 0 || self.progress.create_request.is_some() {
            return;
        }
        if self.progress.begun {
            self.send_progress(WorkDoneProgress::End(WorkDoneProgressEnd {
                message: Some("Indexed".to_string()),
            }));
        }
        self.progress.token = None;
        self.progress.begun = false;
        self.progress.total = 0;
        self.progress.done = 0;
        self.progress.last_percent = 0;
    }

    pub(crate) fn log(&self, kind: MessageType, message: String) {
        let _ = self.send(Notification::new(
            LogMessage::METHOD.to_string(),
            LogMessageParams { typ: kind, message },
        ));
    }

    fn internal(&mut self, event: Internal) -> Result<(), BoxError> {
        match event {
            Internal::Project { root, event } => match event {
                IndexEvent::Discovered(count) => self.job_progress(0, count),
                IndexEvent::Files(files) => {
                    self.given_cache.clear();
                    let count = files.len();
                    if let Some(project) = self.workspace.project_by_root(&root) {
                        project.apply(files);
                    }
                    self.job_progress(count, 0);
                }
                IndexEvent::Persisted(moved) => {
                    if let Some(project) = self.workspace.project_by_root(&root) {
                        for (path, source) in moved {
                            project.index.move_to_cache(&path, source);
                        }
                    }
                }
                IndexEvent::Finished(stats) => {
                    self.workspace.indexed.insert(root.clone());
                    self.resync_open_documents();
                    for uri in self.documents.uris() {
                        self.mark_dirty(uri);
                    }
                    self.log(
                        MessageType::INFO,
                        format!(
                            "Indexed {}: {} files, {} parsed, {} from cache, {} ms",
                            root.display(),
                            stats.files,
                            stats.parsed,
                            stats.from_cache,
                            stats.total.as_millis()
                        ),
                    );
                    self.job_finished();
                    self.refresh_pulled_diagnostics()?;
                    self.refresh_editor_features()?;
                }
            },
            Internal::StubsLocated(dir) => {
                self.workspace.stubs_dir = Some(dir);
                self.workspace.stubs.clear();
            }
            Internal::StubsFailed(message) => {
                self.log(
                    MessageType::WARNING,
                    format!("The PHP standard library stubs are not available: {message}"),
                );
                self.job_finished();
            }
            Internal::StubsEvent(event) => match event {
                IndexEvent::Discovered(count) => self.job_progress(0, count),
                IndexEvent::Files(files) => {
                    let count = files.len();
                    self.workspace.add_stub_files(files);
                    self.job_progress(count, 0);
                }
                IndexEvent::Persisted(moved) => self.workspace.move_stubs_to_cache(moved),
                IndexEvent::Finished(stats) => {
                    self.workspace.apply_stubs();
                    self.resync_open_documents();
                    self.log(
                        MessageType::INFO,
                        format!("Read {} stub files in {} ms", stats.files, stats.total.as_millis()),
                    );
                    self.job_finished();
                    for uri in self.documents.uris() {
                        self.mark_dirty(uri);
                    }
                    self.refresh_pulled_diagnostics()?;
                    self.refresh_editor_features()?;
                }
            },
        }
        Ok(())
    }

    /// An open document's declarations are the ones in the index, whatever the disk or the indexer says.
    fn resync_open_documents(&mut self) {
        for uri in self.documents.uris() {
            if let Some(document) = self.documents.get_mut(&uri) {
                document.indexed_version = None;
            }
            self.sync_symbols(&uri);
        }
    }

    /// Puts the declarations of an open document into the index, when they changed since last time.
    pub(crate) fn sync_symbols(&mut self, uri: &Uri) {
        let Some(path) = uri_to_path(uri) else {
            return;
        };
        let Some(document) = self.documents.get_mut(uri) else {
            return;
        };
        if document.indexed_version == Some(document.version) {
            return;
        }
        if document.twig || document.yaml {
            document.indexed_version = Some(document.version);
            let project = self.workspace.project_for_mut(&path);
            project.words.update(&path, &document.text);
            project
                .index
                .set_open_text(&path, Some(std::sync::Arc::from(document.text.as_str())));
            return;
        }
        let symbols = php_index::extract::extract(
            &document.parse().syntax(),
            php_index::extract::ExtractOptions::default(),
        );
        document.indexed_version = Some(document.version);
        let project = self.workspace.project_for_mut(&path);
        let origin = project.origin_of(&path);
        if origin == php_index::Origin::Project {
            project.words.update(&path, &document.text);
        }
        if project.index.frameworks().any() {
            project
                .index
                .set_open_text(&path, Some(std::sync::Arc::from(document.text.as_str())));
        }
        project.index.set_file(path, origin, std::sync::Arc::new(symbols));
    }

    /// A closed document goes back to what the disk says.
    fn reindex_from_disk(&mut self, uri: &Uri) {
        let Some(path) = uri_to_path(uri) else {
            return;
        };
        let project = self.workspace.project_for_mut(&path);
        let origin = project.origin_of(&path);
        if origin == php_index::Origin::Project {
            project.words.update_from_disk(&path);
        }
        project.index.set_open_text(&path, None);
        if path
            .extension()
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("php"))
        {
            return;
        }
        match index_file(&path, origin) {
            Some(symbols) => project.index.set_file(path, origin, std::sync::Arc::new(symbols)),
            None => project.index.remove_file(&path),
        }
    }

    fn watched_files_changed(&mut self, params: DidChangeWatchedFilesParams) -> Result<(), BoxError> {
        self.given_cache.clear();
        let mut composer_roots: Vec<PathBuf> = Vec::new();
        let mut folders: Vec<PathBuf> = Vec::new();
        for change in params.changes {
            let Some(path) = uri_to_path(&change.uri) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            // `composer dump-autoload` writes the class map again without touching the other two.
            if name == "composer.json" || name == "installed.json" || name == "autoload_classmap.php" {
                if name == "composer.json" {
                    if let Some(folder) = self.workspace.folder_of(&path) {
                        if !folders.contains(&folder) {
                            folders.push(folder);
                        }
                    }
                }
                if let Some(position) = self.workspace.project_position(&path) {
                    let root = self.workspace.projects[position].root.clone();
                    if !composer_roots.contains(&root) {
                        composer_roots.push(root);
                    }
                }
                continue;
            }
            if path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("php"))
            {
                // A `.env`, a translation or a template: what the framework layer read from it is read again.
                let project = self.workspace.project_for_mut(&path);
                if project.index.frameworks().any() && project.contains(&path) {
                    project.index.framework_file_changed(&path);
                }
                let project = self.workspace.project_for(&path);
                let searched =
                    php_analysis::twig::is_template(&path) || php_analysis::yaml::is_config(&project.index, &path);
                if searched && self.documents.get(&change.uri).is_none() {
                    self.workspace.project_for_mut(&path).words.update_from_disk(&path);
                }
                continue;
            }
            if self.documents.get(&change.uri).is_some() {
                continue;
            }
            let project = self.workspace.project_for_mut(&path);
            let origin = project.origin_of(&path);
            match origin {
                php_index::Origin::Project => project.words.update_from_disk(&path),
                php_index::Origin::Vendor => project.package_words.update_from_disk(&path),
                php_index::Origin::Stub => {}
            }
            if change.typ == FileChangeType::DELETED {
                project.index.remove_file(&path);
            } else if let Some(symbols) = index_file(&path, origin) {
                project.index.set_file(path, origin, std::sync::Arc::new(symbols));
            }
        }
        let mut change = FolderChange::default();
        for folder in folders {
            let found = self.workspace.sync_nested(&folder);
            change.start.extend(found.start);
            change.restart.extend(found.restart);
        }
        for root in composer_roots {
            let known = self.workspace.projects.iter().any(|project| project.root == root);
            if known && !change.start.contains(&root) && !change.restart.contains(&root) {
                change.restart.push(root);
            }
        }
        self.apply_folder_change(change);
        Ok(())
    }

    /// `composer.json` or the installed packages changed: read them again and index from scratch.
    fn restart_project(&mut self, root: &Path) {
        if !self.workspace.projects.iter().any(|project| project.root == root) {
            return;
        }
        let default_level = self.workspace.default_level;
        let stubs: Vec<StubFile> = std::mem::take(&mut self.workspace.stubs);
        let stubs_loaded = self.workspace.stubs_loaded;
        self.workspace.indexed.remove(root);
        if let Some(project) = self.workspace.project_by_root(root) {
            project.reload_composer(default_level);
            project.reset_index();
            project.words = php_index::words::WordIndex::default();
            project.package_words = php_index::words::WordIndex::default();
            if stubs_loaded {
                let extensions = project.extensions();
                project.index.set_stubs(&stubs, &extensions);
            }
        }
        self.workspace.stubs = stubs;
        self.workspace.start_project_job(root, &self.internal_sender);
        self.job_started();
        self.resync_open_documents();
        for uri in self.documents.uris() {
            self.mark_dirty(uri);
        }
    }

    // Configuration --------------------------------------------------------------------------

    /// New settings arrived. A client that answers `workspace/configuration` is asked again for every
    /// document, since its answer may differ per folder; one that only pushes sets the default.
    fn configuration_changed(&mut self, settings: &Value) -> Result<(), BoxError> {
        let pushed = Settings::from_value(settings);
        if pushed.hint_parameter_names.is_some() {
            self.settings.hint_parameter_names = pushed.hint_parameter_names;
        }
        if pushed.hint_closure_types.is_some() {
            self.settings.hint_closure_types = pushed.hint_closure_types;
        }
        if pushed.hint_property_types.is_some() {
            self.settings.hint_property_types = pushed.hint_property_types;
        }
        if pushed.inspections.is_some() {
            self.settings.inspections = pushed.inspections;
        }
        if pushed.format.is_some() {
            self.settings.format = pushed.format;
        }
        if pushed.usages_packages.is_some() {
            self.settings.usages_packages = pushed.usages_packages;
        }
        self.refresh_editor_features()?;
        if pushed.php_version.is_some() || !self.configuration_support {
            self.settings.php_version = pushed.php_version;
            self.workspace
                .set_default_level(pushed.php_version.unwrap_or(PhpVersion::DEFAULT));
        }
        if self.configuration_support {
            for uri in self.documents.uris() {
                self.request_configuration(&uri)?;
            }
        }
        for uri in self.documents.uris() {
            self.mark_dirty(uri);
        }
        Ok(())
    }

    fn request_configuration(&mut self, uri: &Uri) -> Result<(), BoxError> {
        if !self.configuration_support {
            return Ok(());
        }
        self.next_request_id += 1;
        let id = RequestId::from(self.next_request_id);
        let params = ConfigurationParams {
            items: vec![ConfigurationItem {
                scope_uri: Some(uri.clone()),
                section: Some(SECTION.to_string()),
            }],
        };
        self.pending_configuration.insert(id.clone(), uri.clone());
        self.send(Request::new(id, WorkspaceConfiguration::METHOD.to_string(), params))
    }

    fn response(&mut self, response: Response) -> Result<(), BoxError> {
        if self.progress.create_request.as_ref() == Some(&response.id) {
            self.progress.create_request = None;
            if response.response_result.is_err() {
                self.progress.token = None;
            } else {
                self.progress.begun = true;
                self.progress.last_percent = self.progress_percent();
                self.send_progress(WorkDoneProgress::Begin(WorkDoneProgressBegin {
                    title: "Indexing PHP files".to_string(),
                    cancellable: Some(false),
                    message: None,
                    percentage: Some(self.progress.last_percent),
                }));
                // The work was done before the client answered: report it as done right away.
                if self.progress.running_jobs == 0 {
                    self.send_progress(WorkDoneProgress::End(WorkDoneProgressEnd {
                        message: Some("Indexed".to_string()),
                    }));
                    self.progress.token = None;
                    self.progress.begun = false;
                }
            }
            return Ok(());
        }
        let Some(uri) = self.pending_configuration.remove(&response.id) else {
            return Ok(());
        };
        let Some(document) = self.documents.get_mut(&uri) else {
            return Ok(());
        };
        let answer = response
            .response_result
            .ok()
            .and_then(|result| result.as_array().and_then(|items| items.first().cloned()))
            .map(|item| Settings::from_value(&item));
        let level = answer.as_ref().and_then(|settings| settings.php_version);
        let format = answer.as_ref().and_then(|settings| settings.format.clone());
        document.format = format;
        document.usages_packages = answer.as_ref().and_then(|settings| settings.usages_packages);
        let inspections = answer.and_then(|settings| settings.inspections);
        if document.level != level || document.inspections != inspections {
            document.level = level;
            document.inspections = inspections;
            self.mark_dirty(uri);
            self.refresh_pulled_diagnostics()?;
        }
        Ok(())
    }

    /// What an editor shows from the index (colors, hints) is out of date: ask it to ask again.
    fn refresh_editor_features(&mut self) -> Result<(), BoxError> {
        if self.semantic_refresh_support {
            self.next_request_id += 1;
            let id = RequestId::from(self.next_request_id);
            self.send(Request::new(id, SemanticTokensRefresh::METHOD.to_string(), ()))?;
        }
        if self.inlay_refresh_support {
            self.next_request_id += 1;
            let id = RequestId::from(self.next_request_id);
            self.send(Request::new(id, InlayHintRefreshRequest::METHOD.to_string(), ()))?;
        }
        Ok(())
    }

    fn refresh_pulled_diagnostics(&mut self) -> Result<(), BoxError> {
        if self.pull_diagnostics && self.diagnostic_refresh_support {
            self.next_request_id += 1;
            let id = RequestId::from(self.next_request_id);
            self.send(Request::new(id, WorkspaceDiagnosticRefresh::METHOD.to_string(), ()))?;
        }
        Ok(())
    }

    fn mark_dirty(&mut self, uri: Uri) {
        if !self.dirty.contains(&uri) {
            self.dirty.push(uri);
        }
    }

    fn publish_dirty(&mut self) -> Result<(), BoxError> {
        if self.pull_diagnostics {
            self.dirty.clear();
            return Ok(());
        }
        for uri in std::mem::take(&mut self.dirty) {
            let version = self.documents.get_mut(&uri).map(|document| document.version);
            if let Some(items) = self.diagnostics_of(&uri) {
                self.publish(uri, version, items)?;
            }
        }
        Ok(())
    }

    fn publish(&self, uri: Uri, version: Option<i32>, diagnostics: Vec<lsp_types::Diagnostic>) -> Result<(), BoxError> {
        self.send(Notification::new(
            PublishDiagnostics::METHOD.to_string(),
            PublishDiagnosticsParams::new(uri, diagnostics, version),
        ))
    }
}
