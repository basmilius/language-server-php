//! The requests that read the index: hover, navigation, workspace symbols and completion.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionItemTag, CompletionParams,
    CompletionResponse, Documentation, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    Location, MarkupContent, MarkupKind, SymbolInformation, SymbolKind, SymbolTag, TextEdit, Uri,
    WorkspaceSymbolParams, WorkspaceSymbolResponse,
};
use php_analysis::completion::{CompletionOptions, ItemKind, complete, resolve_documentation};
use php_analysis::nav::Place;
use php_analysis::target::Target;
use php_analysis::workspace_symbols::workspace_symbols;
use php_analysis::{Analyzer, LineIndex};
use php_index::indexer::index_file;
use php_index::{Origin, Project};
use php_syntax::{TextRange, TextSize};
use serde_json::{Value, json};

use crate::convert::Mapper;
use crate::paths::{path_to_uri, uri_to_path};
use crate::server::Server;

fn range_of(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end.max(start)))
}

/// The text of files that results point into, read once per request.
pub(crate) struct TextCache<'a> {
    server: &'a Server<'a>,
    current: Option<&'a Uri>,
    read: HashMap<PathBuf, Option<(String, LineIndex)>>,
}

impl<'a> TextCache<'a> {
    pub(crate) fn new(server: &'a Server<'a>, current: Option<&'a Uri>) -> TextCache<'a> {
        TextCache {
            server,
            current,
            read: HashMap::new(),
        }
    }

    pub(crate) fn location(&mut self, place: &Place) -> Option<Location> {
        let (uri, range) = match &place.path {
            None => {
                let document = self.server.documents.get(self.current?)?;
                let mapper = Mapper {
                    text: &document.text,
                    index: &document.index,
                    encoding: self.server.encoding,
                };
                (
                    self.current?.clone(),
                    mapper.range(range_of(place.span.start, place.span.end)),
                )
            }
            Some(path) => {
                let uri = path_to_uri(path)?;
                let encoding = self.server.encoding;
                if let Some(document) = self.server.documents.get(&uri) {
                    let mapper = Mapper {
                        text: &document.text,
                        index: &document.index,
                        encoding,
                    };
                    let range = mapper.range(range_of(place.span.start, place.span.end));
                    return Some(Location { uri, range });
                }
                let entry = self.read.entry(path.clone()).or_insert_with(|| {
                    let bytes = std::fs::read(path).ok()?;
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let index = LineIndex::new(&text);
                    Some((text, index))
                });
                let (text, index) = entry.as_ref()?;
                let mapper = Mapper { text, index, encoding };
                (
                    uri,
                    mapper.range(range_of(
                        place.span.start.min(text.len() as u32),
                        place.span.end.min(text.len() as u32),
                    )),
                )
            }
        };
        Some(Location { uri, range })
    }
}

fn item_kind(kind: ItemKind) -> CompletionItemKind {
    match kind {
        ItemKind::Class => CompletionItemKind::CLASS,
        ItemKind::Interface => CompletionItemKind::INTERFACE,
        ItemKind::Trait => CompletionItemKind::STRUCT,
        ItemKind::Enum => CompletionItemKind::ENUM,
        ItemKind::EnumMember => CompletionItemKind::ENUM_MEMBER,
        ItemKind::Function => CompletionItemKind::FUNCTION,
        ItemKind::Method => CompletionItemKind::METHOD,
        ItemKind::Property => CompletionItemKind::PROPERTY,
        ItemKind::Constant => CompletionItemKind::CONSTANT,
        ItemKind::Variable => CompletionItemKind::VARIABLE,
        ItemKind::Keyword => CompletionItemKind::KEYWORD,
        ItemKind::Module => CompletionItemKind::MODULE,
        ItemKind::Parameter => CompletionItemKind::FIELD,
    }
}

fn symbol_kind(kind: php_analysis::SymbolKind) -> SymbolKind {
    use php_analysis::SymbolKind as Kind;
    match kind {
        Kind::Namespace => SymbolKind::NAMESPACE,
        Kind::Class => SymbolKind::CLASS,
        Kind::Interface => SymbolKind::INTERFACE,
        Kind::Trait => SymbolKind::STRUCT,
        Kind::Enum => SymbolKind::ENUM,
        Kind::Method => SymbolKind::METHOD,
        Kind::Constructor => SymbolKind::CONSTRUCTOR,
        Kind::Property => SymbolKind::PROPERTY,
        Kind::Constant => SymbolKind::CONSTANT,
        Kind::EnumMember => SymbolKind::ENUM_MEMBER,
        Kind::Function => SymbolKind::FUNCTION,
    }
}

fn markdown(value: String) -> MarkupContent {
    MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    }
}

impl Server<'_> {
    /// Brings the open document's declarations up to date and runs an analysis at a position.
    fn with_analyzer<R>(
        &mut self,
        uri: &Uri,
        position: lsp_types::Position,
        run: impl FnOnce(&Analyzer<'_>, u32, &Project, &Mapper<'_>) -> R,
    ) -> Option<R> {
        self.sync_symbols(uri);
        let path = uri_to_path(uri);
        let encoding = self.encoding;
        let document = self.documents.get_mut(uri)?;
        let root = document.parse().syntax();
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        let offset = u32::from(mapper.offset(position));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let analyzer = Analyzer::new(&project.index, &root, offset);
        Some(run(&analyzer, offset, project, &mapper))
    }

    /// A class the index has not seen may still be one Composer's autoload maps point at: read that
    /// file now. Returns whether anything was added.
    fn load_missing_classes(&mut self, uri: &Uri, position: lsp_types::Position) -> bool {
        let missing: Vec<String> = self
            .with_analyzer(uri, position, |analyzer, offset, _, _| {
                analyzer
                    .targets_at(offset)
                    .into_iter()
                    .filter_map(|found| match found.target {
                        Target::Class(name) if analyzer.index.class(&name).is_none() => Some(name),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let Some(path) = uri_to_path(uri) else {
            return false;
        };
        let project = self.workspace.project_for_mut(&path);
        let mut loaded = false;
        for name in missing {
            let candidates = project
                .composer
                .as_ref()
                .map(|composer| composer.class_candidates(&name))
                .unwrap_or_default();
            for candidate in candidates {
                if !candidate.is_file() || project.index.contains_file(&candidate) {
                    continue;
                }
                let origin = project.origin_of(&candidate);
                if let Some(symbols) = index_file(&candidate, origin) {
                    project.index.set_file(candidate, origin, std::sync::Arc::new(symbols));
                    loaded = true;
                }
            }
        }
        loaded
    }

    /// The hover over a name or a piece of PHP in a Blade template.
    fn blade_hover(&mut self, uri: &Uri, position: lsp_types::Position) -> Option<Hover> {
        let path = uri_to_path(uri);
        let given = self.template_given(uri);
        let encoding = self.encoding;
        let document = self.documents.get(uri)?;
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        let offset = u32::from(mapper.offset(position));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let hover = php_analysis::blade::hover_at(&project.index, path.as_deref(), &document.text, &given, offset)?;
        Some(Hover {
            contents: HoverContents::Markup(markdown(hover.markdown)),
            range: Some(mapper.range(hover.range)),
        })
    }

    /// Where a name or a piece of PHP in a Blade template is declared.
    fn blade_definition(&mut self, params: &GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        let position = &params.text_document_position_params;
        let uri = &position.text_document.uri;
        let path = uri_to_path(uri);
        let given = self.template_given(uri);
        let encoding = self.encoding;
        let document = self.documents.get(uri)?;
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        let offset = u32::from(mapper.offset(position.position));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let places =
            php_analysis::blade::definitions_at(&project.index, path.as_deref(), &document.text, &given, offset);
        let mut sources = TextCache::new(self, Some(uri));
        let locations: Vec<Location> = places.iter().filter_map(|place| sources.location(place)).collect();
        (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
    }

    pub(crate) fn hover(&mut self, params: HoverParams) -> Option<Hover> {
        let position = params.text_document_position_params;
        if self
            .documents
            .get(&position.text_document.uri)
            .is_some_and(|document| document.blade)
        {
            return self.blade_hover(&position.text_document.uri, position.position);
        }
        self.load_missing_classes(&position.text_document.uri, position.position);
        self.with_analyzer(
            &position.text_document.uri,
            position.position,
            |analyzer, offset, _, mapper| {
                let hover = analyzer.hover(offset)?;
                Some(Hover {
                    contents: HoverContents::Markup(markdown(hover.markdown)),
                    range: Some(mapper.range(hover.range)),
                })
            },
        )?
    }

    fn navigate(
        &mut self,
        params: GotoDefinitionParams,
        query: impl FnOnce(&Analyzer<'_>, u32) -> Vec<Place>,
    ) -> Option<GotoDefinitionResponse> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let places = self.with_analyzer(&uri, position.position, |analyzer, offset, _, _| {
            query(analyzer, offset)
        })?;
        let mut sources = TextCache::new(self, Some(&uri));
        let locations: Vec<Location> = places.iter().filter_map(|place| sources.location(place)).collect();
        (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations))
    }

    pub(crate) fn definition(&mut self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        if self
            .documents
            .get(&params.text_document_position_params.text_document.uri)
            .is_some_and(|document| document.blade)
        {
            return self.blade_definition(&params);
        }
        self.navigate(params, |analyzer, offset| analyzer.definitions(offset))
    }

    pub(crate) fn type_definition(&mut self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        self.navigate(params, |analyzer, offset| analyzer.type_definitions(offset))
    }

    pub(crate) fn implementation(&mut self, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
        self.navigate(params, |analyzer, offset| analyzer.implementations(offset))
    }

    pub(crate) fn workspace_symbols(&mut self, params: WorkspaceSymbolParams) -> Option<WorkspaceSymbolResponse> {
        const LIMIT: usize = 200;
        let mut found = Vec::new();
        let projects = self
            .workspace
            .projects
            .iter()
            .chain(std::iter::once(&self.workspace.loose));
        for project in projects {
            if project.index.file_count() == 0 {
                continue;
            }
            found.extend(workspace_symbols(&project.index, &params.query, LIMIT));
            if self.workspace.projects.len() == 1 {
                break;
            }
        }
        // Projects that hold the same package or the standard library each report their own copy.
        let mut shared = HashSet::new();
        found.retain(|symbol| {
            symbol.origin == Origin::Project
                || shared.insert((symbol.kind, symbol.container.clone(), symbol.name.clone()))
        });
        found.truncate(LIMIT);
        let encoding = self.encoding;
        let mut read: HashMap<PathBuf, Option<(String, LineIndex)>> = HashMap::new();
        let mut symbols = Vec::new();
        for symbol in found {
            let Some(uri) = path_to_uri(&symbol.path) else {
                continue;
            };
            let range = match self.documents.get(&uri) {
                Some(document) => Mapper {
                    text: &document.text,
                    index: &document.index,
                    encoding,
                }
                .range(range_of(symbol.span.start, symbol.span.end)),
                None => {
                    let entry = read.entry(symbol.path.clone()).or_insert_with(|| {
                        let text = String::from_utf8_lossy(&std::fs::read(&symbol.path).ok()?).into_owned();
                        let index = LineIndex::new(&text);
                        Some((text, index))
                    });
                    let Some((text, index)) = entry.as_ref() else {
                        continue;
                    };
                    let length = text.len() as u32;
                    Mapper { text, index, encoding }
                        .range(range_of(symbol.span.start.min(length), symbol.span.end.min(length)))
                }
            };
            #[allow(deprecated)]
            symbols.push(SymbolInformation {
                name: symbol.name,
                kind: symbol_kind(symbol.kind),
                tags: symbol.deprecated.then(|| vec![SymbolTag::DEPRECATED]),
                deprecated: None,
                location: Location { uri, range },
                container_name: symbol.container,
            });
        }
        Some(WorkspaceSymbolResponse::Flat(symbols))
    }

    pub(crate) fn completion(&mut self, params: CompletionParams) -> Option<CompletionResponse> {
        let position = params.text_document_position;
        let uri = position.text_document.uri;
        self.sync_symbols(&uri);
        let path = uri_to_path(&uri);
        let given = self.template_given(&uri);
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding,
        };
        let offset = u32::from(mapper.offset(position.position));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let list = if document.blade {
            php_analysis::blade::complete_at(
                &project.index,
                path.as_deref(),
                &document.text,
                &given,
                offset,
                CompletionOptions::default(),
            )
            .unwrap_or_default()
        } else {
            complete(&project.index, &document.text, offset, CompletionOptions::default())
        };
        let root = project.root.to_string_lossy().into_owned();
        let edit = |edit: &php_analysis::completion::TextEdit| TextEdit {
            range: mapper.range(range_of(edit.start, edit.end)),
            new_text: edit.new_text.clone(),
        };
        let items = list
            .items
            .iter()
            .map(|item| CompletionItem {
                label: item.label.clone(),
                kind: Some(item_kind(item.kind)),
                label_details: item.description.as_ref().map(|description| CompletionItemLabelDetails {
                    detail: None,
                    description: Some(description.clone()),
                }),
                detail: item.detail.clone(),
                tags: item.deprecated.then(|| vec![CompletionItemTag::DEPRECATED]),
                sort_text: Some(item.sort_text.clone()),
                filter_text: item.filter_text.clone(),
                text_edit: Some(lsp_types::CompletionTextEdit::Edit(edit(&item.edit))),
                additional_text_edits: (!item.additional_edits.is_empty())
                    .then(|| item.additional_edits.iter().map(edit).collect()),
                data: item.data.as_ref().map(|key| json!({ "project": root, "key": key })),
                ..CompletionItem::default()
            })
            .collect();
        Some(CompletionResponse::List(lsp_types::CompletionList {
            is_incomplete: list.incomplete,
            items,
        }))
    }

    pub(crate) fn resolve_completion(&mut self, mut item: CompletionItem) -> Option<CompletionItem> {
        let data = item.data.clone()?;
        let key = data.get("key").and_then(Value::as_str)?;
        let root = data.get("project").and_then(Value::as_str).unwrap_or_default();
        let project = self
            .workspace
            .projects
            .iter()
            .find(|project| project.root == Path::new(root))
            .unwrap_or(&self.workspace.loose);
        if let Some(text) = resolve_documentation(&project.index, key) {
            item.documentation = Some(Documentation::MarkupContent(markdown(text)));
        }
        Some(item)
    }
}
