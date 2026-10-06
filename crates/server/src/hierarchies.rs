//! Call hierarchy and type hierarchy over the index and the files of the project.

use std::path::PathBuf;

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem, CallHierarchyOutgoingCall,
    CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams, Range, SymbolKind, SymbolTag, TypeHierarchyItem,
    TypeHierarchyPrepareParams, TypeHierarchySubtypesParams, TypeHierarchySupertypesParams, Uri,
};
use php_analysis::hierarchy::{
    CallItem, CallSymbol, TypeItem, call_item, incoming_calls, outgoing_calls, prepare_call_hierarchy,
    prepare_type_hierarchy, subtypes, supertypes,
};
use php_analysis::nav::Place;
use php_index::{ClassKind, Span};
use serde_json::{Value, json};

use crate::convert::Mapper;
use crate::features::TextCache;
use crate::paths::uri_to_path;
use crate::server::Server;
use crate::usages::ProjectSources;

fn symbol_data(symbol: &CallSymbol) -> Value {
    match symbol {
        CallSymbol::Function(name) => json!({ "kind": "function", "name": name }),
        CallSymbol::Method { class, name } => json!({ "kind": "method", "class": class, "name": name }),
        CallSymbol::File(path) => json!({ "kind": "file", "path": path }),
    }
}

fn symbol_from_data(data: &Value) -> Option<CallSymbol> {
    let text = |key: &str| data.get(key).and_then(Value::as_str).map(str::to_string);
    match data.get("kind")?.as_str()? {
        "function" => Some(CallSymbol::Function(text("name")?)),
        "method" => Some(CallSymbol::Method {
            class: text("class")?,
            name: text("name")?,
        }),
        "file" => Some(CallSymbol::File(PathBuf::from(text("path")?))),
        _ => None,
    }
}

fn class_symbol_kind(kind: ClassKind) -> SymbolKind {
    match kind {
        ClassKind::Class => SymbolKind::CLASS,
        ClassKind::Interface => SymbolKind::INTERFACE,
        ClassKind::Trait => SymbolKind::STRUCT,
        ClassKind::Enum => SymbolKind::ENUM,
    }
}

fn short(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

impl Server<'_> {
    fn range_in(texts: &mut TextCache<'_>, path: &std::path::Path, span: Span) -> Option<(Uri, Range)> {
        let place = Place {
            path: Some(path.to_path_buf()),
            span,
        };
        let location = texts.location(&place)?;
        Some((location.uri, location.range))
    }

    fn call_hierarchy_item(texts: &mut TextCache<'_>, item: &CallItem) -> Option<CallHierarchyItem> {
        let (uri, range) = Self::range_in(texts, &item.path, item.span)?;
        let (_, selection_range) = Self::range_in(texts, &item.path, item.name_span)?;
        let (name, kind, detail) = match &item.symbol {
            CallSymbol::Function(name) => (
                short(name).to_string(),
                SymbolKind::FUNCTION,
                name.rsplit_once('\\').map(|(namespace, _)| namespace.to_string()),
            ),
            CallSymbol::Method { class, name } => (
                name.clone(),
                if name.eq_ignore_ascii_case("__construct") {
                    SymbolKind::CONSTRUCTOR
                } else {
                    SymbolKind::METHOD
                },
                Some(class.clone()),
            ),
            CallSymbol::File(path) => (
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                SymbolKind::FILE,
                None,
            ),
        };
        Some(CallHierarchyItem {
            name,
            kind,
            tags: item.deprecated.then(|| vec![SymbolTag::DEPRECATED]),
            detail,
            uri,
            range,
            selection_range,
            data: Some(symbol_data(&item.symbol)),
        })
    }

    pub(crate) fn prepare_call_hierarchy(
        &mut self,
        params: CallHierarchyPrepareParams,
    ) -> Option<Vec<CallHierarchyItem>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let path = uri_to_path(&uri);
        self.sync_symbols(&uri);
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let root = document.parse().syntax();
        let offset = u32::from(
            Mapper {
                text: &document.text,
                index: &document.index,
                encoding,
            }
            .offset(position.position),
        );
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let items = prepare_call_hierarchy(&project.index, &root, offset);
        let mut texts = TextCache::new(self, Some(&uri));
        Some(
            items
                .iter()
                .filter_map(|item| Self::call_hierarchy_item(&mut texts, item))
                .collect(),
        )
    }

    pub(crate) fn incoming_calls(
        &mut self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Option<Vec<CallHierarchyIncomingCall>> {
        let symbol = symbol_from_data(params.item.data.as_ref()?)?;
        let path = uri_to_path(&params.item.uri)?;
        self.ensure_words(&path);
        let packages = self.package_scope(&params.item.uri, &path);
        let open = self.documents.texts();
        let project = self.workspace.project_for(&path);
        let sources =
            ProjectSources::new(open, &project.words).with_packages(packages.then_some(&project.package_words));
        let calls = incoming_calls(&project.index, &sources, &symbol);
        let mut texts = TextCache::new(self, None);
        let mut out = Vec::new();
        for call in &calls {
            let Some(from) = Self::call_hierarchy_item(&mut texts, &call.from) else {
                continue;
            };
            let from_ranges = call
                .ranges
                .iter()
                .filter_map(|range| {
                    let span = Span {
                        start: u32::from(range.start()),
                        end: u32::from(range.end()),
                    };
                    Self::range_in(&mut texts, &call.from.path, span).map(|(_, range)| range)
                })
                .collect();
            out.push(CallHierarchyIncomingCall { from, from_ranges });
        }
        Some(out)
    }

    pub(crate) fn outgoing_calls(
        &mut self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Option<Vec<CallHierarchyOutgoingCall>> {
        let symbol = symbol_from_data(params.item.data.as_ref()?)?;
        let path = uri_to_path(&params.item.uri)?;
        let open = self.documents.texts();
        let project = self.workspace.project_for(&path);
        let item = call_item(&project.index, &symbol)?;
        let text = match open.get(&item.path) {
            Some(text) => text.clone(),
            None => String::from_utf8_lossy(&std::fs::read(&item.path).ok()?).into_owned(),
        };
        let calls = outgoing_calls(&project.index, &text, &item);
        let mut texts = TextCache::new(self, None);
        let mut out = Vec::new();
        for call in &calls {
            let Some(to) = Self::call_hierarchy_item(&mut texts, &call.to) else {
                continue;
            };
            let from_ranges = call
                .ranges
                .iter()
                .filter_map(|range| {
                    let span = Span {
                        start: u32::from(range.start()),
                        end: u32::from(range.end()),
                    };
                    Self::range_in(&mut texts, &item.path, span).map(|(_, range)| range)
                })
                .collect();
            out.push(CallHierarchyOutgoingCall { to, from_ranges });
        }
        Some(out)
    }

    fn type_hierarchy_item(texts: &mut TextCache<'_>, item: &TypeItem) -> Option<TypeHierarchyItem> {
        let (uri, range) = Self::range_in(texts, &item.path, item.span)?;
        let (_, selection_range) = Self::range_in(texts, &item.path, item.name_span)?;
        Some(TypeHierarchyItem {
            name: short(&item.name).to_string(),
            kind: class_symbol_kind(item.kind),
            tags: None,
            detail: item.name.rsplit_once('\\').map(|(namespace, _)| namespace.to_string()),
            uri,
            range,
            selection_range,
            data: Some(json!({ "name": item.name })),
        })
    }

    pub(crate) fn prepare_type_hierarchy(
        &mut self,
        params: TypeHierarchyPrepareParams,
    ) -> Option<Vec<TypeHierarchyItem>> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let path = uri_to_path(&uri);
        self.sync_symbols(&uri);
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let root = document.parse().syntax();
        let offset = u32::from(
            Mapper {
                text: &document.text,
                index: &document.index,
                encoding,
            }
            .offset(position.position),
        );
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let items = prepare_type_hierarchy(&project.index, &root, offset);
        let mut texts = TextCache::new(self, Some(&uri));
        Some(
            items
                .iter()
                .filter_map(|item| Self::type_hierarchy_item(&mut texts, item))
                .collect(),
        )
    }

    fn related_types(
        &mut self,
        item: &TypeHierarchyItem,
        find: impl Fn(&php_index::Index, &str) -> Vec<TypeItem>,
    ) -> Option<Vec<TypeHierarchyItem>> {
        let name = item.data.as_ref()?.get("name")?.as_str()?.to_string();
        let path = uri_to_path(&item.uri)?;
        let project = self.workspace.project_for(&path);
        let found = find(&project.index, &name);
        let mut texts = TextCache::new(self, None);
        Some(
            found
                .iter()
                .filter_map(|related| Self::type_hierarchy_item(&mut texts, related))
                .collect(),
        )
    }

    pub(crate) fn type_hierarchy_supertypes(
        &mut self,
        params: TypeHierarchySupertypesParams,
    ) -> Option<Vec<TypeHierarchyItem>> {
        self.related_types(&params.item, supertypes)
    }

    pub(crate) fn type_hierarchy_subtypes(
        &mut self,
        params: TypeHierarchySubtypesParams,
    ) -> Option<Vec<TypeHierarchyItem>> {
        self.related_types(&params.item, subtypes)
    }
}
