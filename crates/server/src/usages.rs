//! Find usages and document highlights, over the words of the project and its open documents.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lsc_server::paths::{path_to_uri, uri_to_path};
use lsp_types::{
    DocumentChangeOperation, DocumentChanges, DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams,
    Location, MessageType, OneOf, OptionalVersionedTextDocumentIdentifier, PrepareRenameResponse, ReferenceParams,
    RenameFile, RenameParams, ResourceOp, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};
use php_analysis::nav::Place;
use php_analysis::references::{Current, FileHits, Sources, highlights_at, references_at};
use php_analysis::refs::{Access, Hit, HitKind, Symbol};
use php_analysis::rename::{prepare_rename, rename};
use php_index::words::WordIndex;
use php_index::{Origin, Span};

use crate::documents::ParseDocument;
use crate::features::TextCache;
use crate::server::Server;

/// The files a search reads: open documents as they are in the editor, the others from the disk,
/// narrowed to the ones whose words say they may hold the name.
pub(crate) struct ProjectSources<'a> {
    open: HashMap<PathBuf, String>,
    words: &'a WordIndex,
    /// The words of the installed packages, for a search that reads them too.
    packages: Option<&'a WordIndex>,
}

impl<'a> ProjectSources<'a> {
    pub(crate) fn new(open: HashMap<PathBuf, String>, words: &'a WordIndex) -> ProjectSources<'a> {
        ProjectSources {
            open,
            words,
            packages: None,
        }
    }

    pub(crate) fn with_packages(mut self, packages: Option<&'a WordIndex>) -> ProjectSources<'a> {
        self.packages = packages;
        self
    }
}

impl Sources for ProjectSources<'_> {
    fn candidates(&self, word: &str) -> Vec<PathBuf> {
        let mut found = self.words.candidates(word);
        if let Some(packages) = self.packages {
            found.extend(packages.candidates(word));
        }
        for (path, text) in &self.open {
            if text.to_ascii_lowercase().contains(word) && !found.contains(path) {
                found.push(path.clone());
            }
        }
        found.retain(|path| !self.open.contains_key(path) || self.open[path].to_ascii_lowercase().contains(word));
        found
    }

    fn text(&self, path: &Path) -> Option<String> {
        if let Some(text) = self.open.get(path) {
            return Some(text.clone());
        }
        std::fs::read(path)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }
}

impl Server {
    /// Reads the words of the project's own files, once, before the first search. What the storage
    /// folder kept of an earlier run is read again only for the files that changed.
    pub(crate) fn ensure_words(&mut self, path: &Path) {
        self.ensure_words_of(path, false);
    }

    /// Whether a search from a document also reads the installed packages, and their words when so.
    pub(crate) fn package_scope(&mut self, uri: &Uri, path: &Path) -> bool {
        let wanted = self
            .documents
            .get(uri)
            .and_then(|document| document.state.usages_packages)
            .or(self.settings.usages_packages)
            .unwrap_or(false);
        if wanted {
            self.ensure_words_of(path, true);
        }
        wanted
    }

    /// What the places that render a template give it, for a request about an open template.
    pub(crate) fn template_given(&mut self, uri: &Uri) -> Vec<(String, php_index::Type)> {
        let Some(path) = uri_to_path(uri) else {
            return Vec::new();
        };
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let twig = document.state.twig;
        if !document.state.blade && !twig {
            return Vec::new();
        }
        if let Some(given) = self.given_cache.get(&path) {
            return given.clone();
        }
        self.ensure_words(&path);
        let open = self.documents.texts();
        let project = self.workspace.project_for(&path);
        let sources = ProjectSources::new(open, &project.words);
        let given = if twig {
            php_analysis::twig::data::given(&project.index, &sources, &path)
        } else {
            php_analysis::blade::data::given(&project.index, &sources, &path)
        };
        self.given_cache.insert(path, given.clone());
        given
    }

    fn ensure_words_of(&mut self, path: &Path, packages: bool) {
        let storage = self.workspace.storage.clone();
        // Built from a partial index, the words would miss every file read after this search and
        // stay that way; until the index is complete a search reads the open documents only.
        let root = &self.workspace.project_for(path).root;
        let complete = root.as_os_str().is_empty() || self.workspace.indexed.contains(root);
        let project = self.workspace.project_for_mut(path);
        let (origin, built) = if packages {
            (Origin::Vendor, project.package_words.is_built())
        } else {
            (Origin::Project, project.words.is_built())
        };
        if built || !complete {
            return;
        }
        let mut paths: Vec<PathBuf> = project
            .index
            .files()
            .filter(|file| file.origin == origin)
            .map(|file| file.path.clone())
            .collect();
        let frameworks = project.index.frameworks();
        if !packages && (frameworks.symfony || frameworks.twig) {
            let templates = project
                .index
                .section::<php_index::framework::symfony::templates::Templates>();
            paths.extend(templates.templates.iter().map(|template| template.path.clone()));
        }
        if !packages && frameworks.symfony {
            for folder in ["config", "translations"] {
                paths.extend(
                    project
                        .index
                        .files_below(&project.root.join(folder))
                        .into_iter()
                        .filter(|path| php_analysis::yaml::is_config(&project.index, path)),
                );
            }
        }
        let kept = storage
            .filter(|_| !project.root.as_os_str().is_empty())
            .map(|storage| project.words_path(&storage, packages));
        let words = if packages {
            &mut project.package_words
        } else {
            &mut project.words
        };
        words.build(paths, kept.as_deref());
    }

    pub(crate) fn references(&mut self, params: ReferenceParams) -> Option<Vec<Location>> {
        let position = params.text_document_position;
        let uri = position.text_document.uri;
        let include_declaration = params.context.include_declaration;
        let offset = self.offset_of(&uri, position.position)?;
        if let Some(locations) = self.sql_references(&uri, offset, include_declaration) {
            return Some(locations);
        }
        let path = uri_to_path(&uri)?;
        self.sync_symbols(&uri);
        self.ensure_words(&path);
        let packages = self.package_scope(&uri, &path);
        let open = self.documents.texts();
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let root = document.parse().syntax();
        let offset = u32::from(document.mapper(encoding).offset(position.position));
        let project = self.workspace.project_for(&path);
        let sources =
            ProjectSources::new(open, &project.words).with_packages(packages.then_some(&project.package_words));
        let found = references_at(
            &project.index,
            &sources,
            &Current {
                path: &path,
                text: &document.text,
                root: &root,
            },
            offset,
        )?;
        let mut files = found.files;
        if include_declaration {
            add_foreign_declarations(&project.index, &found.symbols, &mut files);
        } else {
            for file in &mut files {
                file.hits.retain(|hit| hit.kind != HitKind::Declaration);
            }
        }
        let mut texts = TextCache::new(self, Some(&uri));
        let mut locations = Vec::new();
        for file in &files {
            for hit in &file.hits {
                let place = Place {
                    path: Some(file.path.clone()),
                    span: Span {
                        start: u32::from(hit.range.start()),
                        end: u32::from(hit.range.end()),
                    },
                };
                if let Some(location) = texts.location(&place) {
                    locations.push(location);
                }
            }
        }
        Some(locations)
    }

    pub(crate) fn document_highlight(&mut self, params: DocumentHighlightParams) -> Option<Vec<DocumentHighlight>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let offset = self.offset_of(&uri, position.position)?;
        if let Some(highlights) = self.sql_highlights(&uri, offset) {
            return Some(highlights);
        }
        let path = uri_to_path(&uri);
        self.sync_symbols(&uri);
        let blade = self.documents.get(&uri)?.state.blade;
        if blade {
            if let Some(path) = &path {
                self.ensure_words(path);
            }
        }
        let open = if blade { self.documents.texts() } else { HashMap::new() };
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let root = document.parse().syntax();
        let mapper = document.mapper(encoding);
        let offset = u32::from(mapper.offset(position.position));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let current_path = path.unwrap_or_default();
        let sources = ProjectSources::new(open, &project.words);
        let hits = highlights_at(
            &project.index,
            &sources,
            &Current {
                path: &current_path,
                text: &document.text,
                root: &root,
            },
            offset,
        );
        Some(
            hits.iter()
                .map(|hit| DocumentHighlight {
                    range: mapper.range(hit.range),
                    kind: Some(highlight_kind(hit)),
                })
                .collect(),
        )
    }
}

fn highlight_kind(hit: &Hit) -> DocumentHighlightKind {
    match (&hit.symbol, hit.access) {
        (Symbol::Variable { .. } | Symbol::Property { .. } | Symbol::ClassConst { .. }, Access::Write) => {
            DocumentHighlightKind::WRITE
        }
        (Symbol::Variable { .. } | Symbol::Property { .. } | Symbol::ClassConst { .. }, Access::Read) => {
            DocumentHighlightKind::READ
        }
        _ => DocumentHighlightKind::TEXT,
    }
}

/// A declaration in a file the search does not read (a package, the standard library) still belongs
/// to the answer when the client asks for it, and so does the place a route, a config key or a
/// translation is declared, which is no usage of it.
fn add_foreign_declarations(index: &php_index::Index, symbols: &[Symbol], files: &mut Vec<FileHits>) {
    for symbol in symbols {
        let query = php_analysis::refs::Query::new(index, symbol.clone());
        for declaration in php_analysis::decl::declarations(index, &query) {
            if declaration.origin == Origin::Project && !matches!(symbol, Symbol::Key { .. }) {
                continue;
            }
            let range =
                php_syntax::TextRange::new(declaration.name_span.start.into(), declaration.name_span.end.into());
            let hit = Hit {
                range,
                kind: HitKind::Declaration,
                access: Access::Read,
                dollar: false,
                via_alias: false,
                symbol: symbol.clone(),
            };
            match files.iter_mut().find(|file| file.path == declaration.path) {
                Some(file) => {
                    if !file.hits.iter().any(|existing| existing.range == range) {
                        file.hits.push(hit);
                    }
                }
                None => files.push(FileHits {
                    path: declaration.path.clone(),
                    hits: vec![hit],
                }),
            }
        }
    }
}

// Rename ---------------------------------------------------------------------------------------

impl Server {
    pub(crate) fn prepare_rename(
        &mut self,
        params: lsp_types::TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>, String> {
        let uri = params.text_document.uri;
        if let Some(offset) = self.offset_of(&uri, params.position) {
            if let Some(prepared) = self.sql_prepare_rename(&uri, offset) {
                return prepared;
            }
        }
        let path = uri_to_path(&uri);
        self.sync_symbols(&uri);
        let given = self.template_given(&uri);
        let encoding = self.encoding;
        let Some(document) = self.documents.get_mut(&uri) else {
            return Ok(None);
        };
        let root = document.parse().syntax();
        let mapper = document.mapper(encoding);
        let offset = u32::from(mapper.offset(params.position));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let prepared = if document.state.blade {
            php_analysis::blade::prepare_rename(&project.index, path.as_deref(), &document.text, &given, offset)?
        } else {
            prepare_rename(&project.index, &root, &document.text, offset)?
        };
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: mapper.range(prepared.range),
            placeholder: prepared.placeholder,
        }))
    }

    // `changes` of a workspace edit is a map keyed by `Uri` in the protocol's own types.
    #[allow(clippy::mutable_key_type)]
    pub(crate) fn rename(&mut self, params: RenameParams) -> Result<Option<WorkspaceEdit>, String> {
        let position = params.text_document_position;
        let uri = position.text_document.uri;
        if let Some(offset) = self.offset_of(&uri, position.position) {
            if let Some(renamed) = self.sql_rename(&uri, offset, &params.new_name) {
                return renamed;
            }
        }
        let Some(path) = uri_to_path(&uri) else {
            return Err("Only files can be renamed".to_string());
        };
        self.sync_symbols(&uri);
        self.ensure_words(&path);
        let open = self.documents.texts();
        let encoding = self.encoding;
        let Some(document) = self.documents.get_mut(&uri) else {
            return Ok(None);
        };
        let root = document.parse().syntax();
        let offset = u32::from(document.mapper(encoding).offset(position.position));
        let project = self.workspace.project_for(&path);
        let sources = ProjectSources::new(open, &project.words);
        let current = Current {
            path: &path,
            text: &document.text,
            root: &root,
        };
        let done = if document.state.blade {
            php_analysis::blade::rename(&project.index, &sources, &current, offset, &params.new_name)?
        } else {
            rename(&project.index, &sources, &current, offset, &params.new_name)?
        };
        let file_move = done.file_rename.as_ref().filter(|moved| {
            project.composer.as_ref().is_none_or(|composer| {
                composer
                    .class_candidates(&moved.class)
                    .iter()
                    .any(|candidate| candidate == &moved.from)
            })
        });
        let file_move = file_move.cloned();
        let mut texts = TextCache::new(self, Some(&uri));
        let mut per_file: Vec<(Uri, Option<i32>, Vec<TextEdit>)> = Vec::new();
        for file in &done.files {
            let mut edits = Vec::new();
            let mut file_uri = None;
            for edit in &file.edits {
                let place = Place {
                    path: Some(file.path.clone()),
                    span: Span {
                        start: u32::from(edit.range.start()),
                        end: u32::from(edit.range.end()),
                    },
                };
                if let Some(location) = texts.location(&place) {
                    file_uri = Some(location.uri);
                    edits.push(TextEdit {
                        range: location.range,
                        new_text: edit.text.clone(),
                    });
                }
            }
            if let Some(file_uri) = file_uri {
                let version = self.documents.get(&file_uri).map(|document| document.version);
                per_file.push((file_uri, version, edits));
            }
        }
        let mut notes = vec![
            "Rename changes names in code, doc comments and the strings that name them. Other strings and comments are not changed.".to_string(),
        ];
        let mut operations: Vec<DocumentChangeOperation> = Vec::new();
        if let Some(moved) = &file_move {
            match (path_to_uri(&moved.from), path_to_uri(&moved.to)) {
                (Some(old_uri), Some(new_uri)) if self.rename_files && self.document_changes => {
                    operations.push(DocumentChangeOperation::Op(ResourceOp::Rename(RenameFile {
                        old_uri,
                        new_uri,
                        options: None,
                        annotation_id: None,
                    })));
                }
                _ => notes.push(format!(
                    "The file {} follows the class name, but the client cannot rename files.",
                    moved.from.display()
                )),
            }
        }
        for note in notes {
            self.log(MessageType::INFO, note);
        }
        if self.document_changes {
            let mut changes: Vec<DocumentChangeOperation> = per_file
                .into_iter()
                .map(|(uri, version, edits)| {
                    DocumentChangeOperation::Edit(TextDocumentEdit {
                        text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
                        edits: edits.into_iter().map(OneOf::Left).collect(),
                    })
                })
                .collect();
            changes.extend(operations);
            return Ok(Some(WorkspaceEdit {
                changes: None,
                document_changes: Some(DocumentChanges::Operations(changes)),
                change_annotations: None,
            }));
        }
        let changes: HashMap<Uri, Vec<TextEdit>> = per_file.into_iter().map(|(uri, _, edits)| (uri, edits)).collect();
        Ok(Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }))
    }
}
