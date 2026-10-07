//! What the analyses of the SQL strings answer, as LSP: diagnostics, completion, hover, definition,
//! signature help, highlights, inlay hints, code actions, references, rename and semantic tokens,
//! each in the document's own positions.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_types::{
    CodeAction, CodeActionKind, CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionList,
    CompletionTextEdit, Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag, DocumentChanges,
    DocumentHighlight, DocumentHighlightKind, Documentation, Hover, HoverContents, InlayHint, InlayHintKind,
    InlayHintLabel, InsertTextFormat, Location, MarkupContent, MarkupKind, NumberOrString, OneOf,
    OptionalVersionedTextDocumentIdentifier, ParameterInformation, ParameterLabel, PrepareRenameResponse,
    RegistrationParams, SignatureHelp, SignatureInformation, TextDocumentEdit, TextEdit, Uri, WorkspaceEdit,
};
use php_analysis::nav::Place;
use php_syntax::{TextRange, TextSize};
use serde_json::json;
use sql_embed::{Access, ActionKind, CompletionOptions, ItemKind, Span, SqlFile, TOKEN_MODIFIERS, TOKEN_TYPES};

use crate::features::TextCache;
use crate::server::Server;

/// The kind of the action that applies every safe fix of the SQL in a document.
pub(crate) const FIX_ALL: &str = "source.fixAll.sql";

/// The token types of the legend: PHP's, then the ones only SQL has. Keywords, strings, numbers
/// and comments of PHP stay with the editor's grammar; inside a string of SQL the editor sees one
/// string, so SQL's own are given.
pub(crate) fn legend_types() -> Vec<&'static str> {
    let mut types: Vec<&'static str> = php_analysis::semantic_tokens::TOKEN_TYPES.to_vec();
    for name in TOKEN_TYPES {
        if !types.contains(name) {
            types.push(name);
        }
    }
    types
}

pub(crate) fn legend_modifiers() -> Vec<&'static str> {
    let mut modifiers: Vec<&'static str> = php_analysis::semantic_tokens::TOKEN_MODIFIERS.to_vec();
    for name in TOKEN_MODIFIERS {
        if !modifiers.contains(name) {
            modifiers.push(name);
        }
    }
    modifiers
}

/// A token of SQL in the legend: its type and modifiers as the PHP legend numbers them.
fn token_in_legend(ty: u32, modifiers: u32, types: &[&str], legend_modifiers: &[&str]) -> Option<(u32, u32)> {
    let name = TOKEN_TYPES.get(ty as usize)?;
    let ty = types.iter().position(|known| known == name)? as u32;
    let mut bits = 0;
    for (bit, modifier) in TOKEN_MODIFIERS.iter().enumerate() {
        if modifiers & (1 << bit) != 0 {
            if let Some(at) = legend_modifiers.iter().position(|known| known == modifier) {
                bits |= 1 << at;
            }
        }
    }
    Some((ty, bits))
}

/// A token of the document in byte offsets, with its place in the legend.
pub(crate) struct Token {
    pub start: u32,
    pub end: u32,
    pub ty: u32,
    pub modifiers: u32,
}

fn range_of(span: Span) -> TextRange {
    TextRange::new(TextSize::from(span.start), TextSize::from(span.end.max(span.start)))
}

fn markdown(value: String) -> MarkupContent {
    MarkupContent {
        kind: MarkupKind::Markdown,
        value,
    }
}

fn completion_kind(kind: ItemKind) -> CompletionItemKind {
    match kind {
        ItemKind::Keyword => CompletionItemKind::KEYWORD,
        ItemKind::Table => CompletionItemKind::CLASS,
        ItemKind::View => CompletionItemKind::INTERFACE,
        ItemKind::Column => CompletionItemKind::FIELD,
        ItemKind::Alias => CompletionItemKind::VARIABLE,
        ItemKind::Schema => CompletionItemKind::MODULE,
        ItemKind::Function => CompletionItemKind::FUNCTION,
        ItemKind::Procedure => CompletionItemKind::METHOD,
        ItemKind::Type => CompletionItemKind::TYPE_PARAMETER,
        ItemKind::Sequence => CompletionItemKind::VALUE,
        ItemKind::Value => CompletionItemKind::ENUM_MEMBER,
        ItemKind::Snippet => CompletionItemKind::SNIPPET,
        ItemKind::Setting => CompletionItemKind::PROPERTY,
    }
}

fn severity(severity: sql_embed::DiagnosticSeverity) -> DiagnosticSeverity {
    match severity {
        sql_embed::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
        sql_embed::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
        sql_embed::DiagnosticSeverity::Information => DiagnosticSeverity::INFORMATION,
        sql_embed::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
    }
}

impl Server {
    /// The diagnostics of the SQL in a PHP document, told apart from PHP's by their source `sql`.
    pub(crate) fn sql_diagnostics(&mut self, uri: &Uri) -> Vec<Diagnostic> {
        let Some(items) = self.sql_injected(uri) else {
            return Vec::new();
        };
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let mapper = document.mapper(self.encoding);
        let mut out = Vec::new();
        for item in items.iter() {
            for found in item.analysis.diagnostics() {
                let mut tags = Vec::new();
                if found.deprecated {
                    tags.push(DiagnosticTag::DEPRECATED);
                }
                if found.unnecessary {
                    tags.push(DiagnosticTag::UNNECESSARY);
                }
                let related: Vec<DiagnosticRelatedInformation> = found
                    .related
                    .iter()
                    .map(|related| DiagnosticRelatedInformation {
                        location: Location::new(uri.clone(), mapper.range(range_of(related.span))),
                        message: related.message.clone(),
                    })
                    .collect();
                out.push(Diagnostic {
                    range: mapper.visible_range(range_of(found.span)),
                    severity: Some(severity(found.severity)),
                    code: Some(NumberOrString::String(found.code.to_string())),
                    source: Some("sql".to_string()),
                    message: found.message,
                    related_information: (!related.is_empty()).then_some(related),
                    tags: (!tags.is_empty()).then_some(tags),
                    data: found.feature.map(|feature| json!({ "feature": feature })),
                    ..Diagnostic::default()
                });
            }
        }
        out
    }

    /// What completes at an offset in SQL, when the offset is in SQL and there is anything.
    pub(crate) fn sql_completion(&mut self, uri: &Uri, offset: u32) -> Option<CompletionList> {
        let (items, position) = self.sql_at(uri, offset)?;
        let options = CompletionOptions {
            snippets: self.snippet_support,
            ..CompletionOptions::default()
        };
        let list = items[position].analysis.completion(offset, options);
        if list.items.is_empty() {
            return None;
        }
        let document = self.documents.get(uri)?;
        let mapper = document.mapper(self.encoding);
        let items = list
            .items
            .into_iter()
            .map(|item| CompletionItem {
                label: item.label,
                kind: Some(completion_kind(item.kind)),
                label_details: item.description.map(|description| CompletionItemLabelDetails {
                    detail: None,
                    description: Some(description),
                }),
                detail: item.detail,
                documentation: item
                    .documentation
                    .map(|text| Documentation::MarkupContent(markdown(text))),
                sort_text: Some(item.sort_text),
                filter_text: item.filter_text,
                insert_text_format: Some(if item.snippet {
                    InsertTextFormat::SNIPPET
                } else {
                    InsertTextFormat::PLAIN_TEXT
                }),
                text_edit: Some(CompletionTextEdit::Edit(TextEdit::new(
                    mapper.range(range_of(item.edit.span)),
                    item.edit.new_text,
                ))),
                ..CompletionItem::default()
            })
            .collect();
        Some(CompletionList {
            is_incomplete: list.incomplete,
            items,
        })
    }

    pub(crate) fn sql_hover(&mut self, uri: &Uri, offset: u32) -> Option<Hover> {
        let (items, position) = self.sql_at(uri, offset)?;
        let hover = items[position].analysis.hover(offset)?;
        let mapper = self.documents.get(uri)?.mapper(self.encoding);
        Some(Hover {
            contents: HoverContents::Markup(markdown(hover.markdown)),
            range: Some(mapper.range(range_of(hover.span))),
        })
    }

    /// Where a name of SQL is defined: in the string, or in a `.sql` file of the workspace. `None`
    /// when the offset is not in SQL; an empty list when it is and nothing is found.
    pub(crate) fn sql_definition(&mut self, uri: &Uri, offset: u32) -> Option<Vec<Location>> {
        let (items, position) = self.sql_at(uri, offset)?;
        let mut places = Vec::new();
        for location in items[position].analysis.definition(offset) {
            match location {
                sql_embed::Location::Fragment { name, .. } => places.push(Place {
                    path: None,
                    span: php_index::Span {
                        start: name.start,
                        end: name.end,
                    },
                }),
                sql_embed::Location::File { path, name, .. } => places.push(Place {
                    path: Some(path),
                    span: php_index::Span {
                        start: name.start,
                        end: name.end,
                    },
                }),
            }
        }
        let mut sources = TextCache::new(self, Some(uri));
        Some(places.iter().filter_map(|place| sources.location(place)).collect())
    }

    pub(crate) fn sql_signature_help(&mut self, uri: &Uri, offset: u32) -> Option<SignatureHelp> {
        let (items, position) = self.sql_at(uri, offset)?;
        let help = items[position].analysis.signature_help(offset)?;
        let active_parameter = help
            .signatures
            .get(help.active_signature)
            .and_then(|signature| signature.active_parameter)
            .map(|active| active as u32);
        Some(SignatureHelp {
            signatures: help
                .signatures
                .into_iter()
                .map(|signature| SignatureInformation {
                    label: signature.label,
                    documentation: signature
                        .documentation
                        .map(|text| Documentation::MarkupContent(markdown(text))),
                    parameters: Some(
                        signature
                            .parameters
                            .into_iter()
                            .map(|parameter| ParameterInformation {
                                label: ParameterLabel::Simple(parameter.label),
                                documentation: None,
                            })
                            .collect(),
                    ),
                    active_parameter: signature.active_parameter.map(|active| active as u32),
                })
                .collect(),
            active_signature: Some(help.active_signature as u32),
            active_parameter,
        })
    }

    pub(crate) fn sql_highlights(&mut self, uri: &Uri, offset: u32) -> Option<Vec<DocumentHighlight>> {
        let (items, position) = self.sql_at(uri, offset)?;
        let mapper = self.documents.get(uri)?.mapper(self.encoding);
        Some(
            items[position]
                .analysis
                .highlights(offset)
                .into_iter()
                .map(|hit| DocumentHighlight {
                    range: mapper.range(range_of(hit.span)),
                    kind: Some(match hit.access {
                        Access::Read => DocumentHighlightKind::READ,
                        Access::Write | Access::Declaration => DocumentHighlightKind::WRITE,
                    }),
                })
                .collect(),
        )
    }

    /// The inlay hints of the SQL strings that start or end in a range.
    pub(crate) fn sql_inlay_hints(&mut self, uri: &Uri, range: TextRange) -> Vec<InlayHint> {
        let Some(items) = self.sql_injected(uri) else {
            return Vec::new();
        };
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let mapper = document.mapper(self.encoding);
        let mut out = Vec::new();
        for item in items.iter() {
            if item.range.intersect(range).is_none() {
                continue;
            }
            for hint in item.analysis.inlay_hints() {
                if !range.contains_inclusive(TextSize::from(hint.offset)) {
                    continue;
                }
                out.push(InlayHint {
                    position: mapper.position(TextSize::from(hint.offset)),
                    label: InlayHintLabel::String(hint.label),
                    kind: Some(InlayHintKind::PARAMETER),
                    text_edits: None,
                    tooltip: None,
                    padding_left: Some(false),
                    padding_right: Some(true),
                    data: None,
                });
            }
        }
        out
    }

    fn sql_workspace_edit(&self, uri: &Uri, edits: Vec<TextEdit>) -> WorkspaceEdit {
        if self.document_changes {
            let version = self.documents.get(uri).map(|document| document.version);
            return WorkspaceEdit {
                document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                    text_document: OptionalVersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version,
                    },
                    edits: edits.into_iter().map(OneOf::Left).collect(),
                }])),
                ..WorkspaceEdit::default()
            };
        }
        #[allow(clippy::mutable_key_type)]
        let changes = HashMap::from([(uri.clone(), edits)]);
        WorkspaceEdit {
            changes: Some(changes),
            ..WorkspaceEdit::default()
        }
    }

    /// The quick fixes and rewrites of the SQL a range is in, and every safe fix of the document's
    /// SQL when the client asks for `source.fixAll`.
    pub(crate) fn sql_code_actions(
        &mut self,
        uri: &Uri,
        range: TextRange,
        diagnostics: &[Diagnostic],
        only: Option<&[CodeActionKind]>,
    ) -> Vec<CodeAction> {
        let Some(items) = self.sql_injected(uri) else {
            return Vec::new();
        };
        let wanted = |kind: &str| crate::actions::wanted(kind, only);
        let fix_all_asked = only.is_some_and(|only| {
            only.iter()
                .any(|asked| FIX_ALL == asked.as_str() || FIX_ALL.starts_with(&format!("{}.", asked.as_str())))
        });
        let (start, end) = (u32::from(range.start()), u32::from(range.end()));
        let mut found: Vec<sql_embed::CodeAction> = Vec::new();
        for item in items.iter() {
            if item.holds(start) && item.holds(end) {
                found.extend(
                    item.analysis
                        .code_actions(Span::new(start, end))
                        .into_iter()
                        .filter(|action| action.kind != ActionKind::FixAll || !fix_all_asked),
                );
            }
        }
        if fix_all_asked {
            let edits: Vec<sql_embed::Edit> = items
                .iter()
                .filter_map(|item| item.analysis.fix_all())
                .flat_map(|action| action.edits)
                .collect();
            if !edits.is_empty() {
                found.push(sql_embed::CodeAction {
                    title: "Fix every SQL problem that has a safe fix".to_string(),
                    kind: ActionKind::FixAll,
                    edits,
                    preferred: false,
                    fixes: None,
                });
            }
        }
        let Some(document) = self.documents.get(uri) else {
            return Vec::new();
        };
        let mapper = document.mapper(self.encoding);
        let mut out = Vec::new();
        for action in found {
            let kind = match action.kind {
                ActionKind::QuickFix => CodeActionKind::QUICKFIX,
                ActionKind::Rewrite => CodeActionKind::REFACTOR_REWRITE,
                ActionKind::FixAll => CodeActionKind::new(FIX_ALL),
            };
            if !wanted(kind.as_str()) {
                continue;
            }
            let fixed = action.fixes.map(|(span, code)| {
                let range = mapper.visible_range(range_of(span));
                diagnostics
                    .iter()
                    .filter(|diagnostic| {
                        diagnostic.range == range
                            && diagnostic.source.as_deref() == Some("sql")
                            && diagnostic.code == Some(NumberOrString::String(code.to_string()))
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            });
            let edits = action
                .edits
                .into_iter()
                .map(|edit| TextEdit::new(mapper.range(range_of(edit.span)), edit.new_text))
                .collect();
            out.push(CodeAction {
                title: action.title,
                kind: Some(kind),
                diagnostics: fixed.filter(|fixed| !fixed.is_empty()),
                edit: Some(self.sql_workspace_edit(uri, edits)),
                is_preferred: action.preferred.then_some(true),
                ..CodeAction::default()
            });
        }
        out
    }

    /// Where the table, column or name of SQL at an offset is named: in the string, in the other
    /// SQL strings of the open documents, and in the `.sql` files of the workspace.
    pub(crate) fn sql_references(&mut self, uri: &Uri, offset: u32, declaration: bool) -> Option<Vec<Location>> {
        let (items, position) = self.sql_at(uri, offset)?;
        let Some(found) = items[position].analysis.references(offset) else {
            return Some(Vec::new());
        };
        let mut places: Vec<(Uri, Span)> = Vec::new();
        let keep = |access: Access| declaration || access != Access::Declaration;
        if found.symbol.is_local() {
            for hit in found.hits.iter().filter(|hit| keep(hit.access)) {
                places.push((uri.clone(), hit.span));
            }
        } else {
            for other in self.documents.uris() {
                let Some(strings) = self.sql_injected(&other) else {
                    continue;
                };
                for item in strings.iter() {
                    for hit in item.analysis.hits(&found.symbol) {
                        if keep(hit.access) {
                            places.push((other.clone(), hit.span));
                        }
                    }
                }
            }
        }
        let mut locations = Vec::new();
        for (target, span) in places {
            let Some(document) = self.documents.get(&target) else {
                continue;
            };
            let mapper = document.mapper(self.encoding);
            locations.push(Location::new(target.clone(), mapper.range(range_of(span))));
        }
        if !found.symbol.is_local() {
            let files: Vec<(PathBuf, &String)> = self
                .sql
                .sql_files()
                .iter()
                .map(|(path, text)| (path.clone(), text))
                .collect();
            let sources: Vec<SqlFile> = files
                .iter()
                .map(|(path, text)| SqlFile {
                    path: path.as_path(),
                    text: text.as_str(),
                })
                .collect();
            let in_files = items[position].analysis.references_in_files(&found.symbol, &sources);
            let mut texts = TextCache::new(self, Some(uri));
            for file in in_files {
                for span in file.spans {
                    let place = Place {
                        path: Some(file.path.clone()),
                        span: php_index::Span {
                            start: span.start,
                            end: span.end,
                        },
                    };
                    if let Some(location) = texts.location(&place) {
                        locations.push(location);
                    }
                }
            }
        }
        Some(locations)
    }

    /// The name of SQL at an offset that rename can change: an alias, a common table expression or
    /// a column alias the string declares itself.
    pub(crate) fn sql_prepare_rename(
        &mut self,
        uri: &Uri,
        offset: u32,
    ) -> Option<Result<Option<PrepareRenameResponse>, String>> {
        let (items, position) = self.sql_at(uri, offset)?;
        let mapper = self.documents.get(uri)?.mapper(self.encoding);
        Some(
            items[position]
                .analysis
                .prepare_rename(offset)
                .map(|(span, placeholder)| {
                    Some(PrepareRenameResponse::RangeWithPlaceholder {
                        range: mapper.range(range_of(span)),
                        placeholder,
                    })
                }),
        )
    }

    pub(crate) fn sql_rename(
        &mut self,
        uri: &Uri,
        offset: u32,
        new_name: &str,
    ) -> Option<Result<Option<WorkspaceEdit>, String>> {
        let (items, position) = self.sql_at(uri, offset)?;
        let edits = match items[position].analysis.rename(offset, new_name) {
            Ok(edits) => edits,
            Err(message) => return Some(Err(message)),
        };
        let mapper = self.documents.get(uri)?.mapper(self.encoding);
        let edits = edits
            .into_iter()
            .map(|edit| TextEdit::new(mapper.range(range_of(edit.span)), edit.new_text))
            .collect();
        Some(Ok(Some(self.sql_workspace_edit(uri, edits))))
    }

    /// The semantic tokens of the SQL strings of a document, or of the ones in a range, in the
    /// legend of the server.
    pub(crate) fn sql_tokens(&mut self, uri: &Uri, range: Option<TextRange>) -> Vec<Token> {
        let Some(items) = self.sql_injected(uri) else {
            return Vec::new();
        };
        let types = legend_types();
        let modifiers = legend_modifiers();
        let mut out = Vec::new();
        for item in items.iter() {
            if range.is_some_and(|range| item.range.intersect(range).is_none()) {
                continue;
            }
            for token in item.analysis.semantic_tokens() {
                let Some((ty, bits)) = token_in_legend(token.ty, token.modifiers, &types, &modifiers) else {
                    continue;
                };
                out.push(Token {
                    start: token.span.start,
                    end: token.span.end,
                    ty,
                    modifiers: bits,
                });
            }
        }
        out
    }

    /// Asks the client to watch the snapshots that were read, so a change reaches the server.
    pub(crate) fn watch_snapshots(&mut self) {
        if !self.watch_support {
            return;
        }
        let watchers: Vec<lsp_types::FileSystemWatcher> = self
            .sql
            .snapshot_paths()
            .iter()
            .filter_map(|path| path.to_str())
            .map(|path| lsp_types::FileSystemWatcher {
                glob_pattern: lsp_types::GlobPattern::String(path.to_string()),
                kind: None,
            })
            .collect();
        if watchers.is_empty() {
            return;
        }
        let options = lsp_types::DidChangeWatchedFilesRegistrationOptions { watchers };
        let Ok(options) = serde_json::to_value(options) else {
            return;
        };
        let method = <lsp_types::notification::DidChangeWatchedFiles as lsp_types::notification::Notification>::METHOD;
        if self.snapshot_watch_registered {
            let _ = self
                .client
                .request::<lsp_types::request::UnregisterCapability>(lsp_types::UnregistrationParams {
                    unregisterations: vec![lsp_types::Unregistration {
                        id: SNAPSHOT_WATCH.to_string(),
                        method: method.to_string(),
                    }],
                });
        }
        self.snapshot_watch_registered = true;
        let _ = self
            .client
            .request::<lsp_types::request::RegisterCapability>(RegistrationParams {
                registrations: vec![lsp_types::Registration {
                    id: SNAPSHOT_WATCH.to_string(),
                    method: method.to_string(),
                    register_options: Some(options),
                }],
            });
    }
}

/// The id of the registration that watches the schema snapshots.
const SNAPSHOT_WATCH: &str = "php-sql-snapshots";

/// The tokens of PHP and of the SQL in its strings as one list in document order: where SQL has a
/// token, PHP's token over the same text gives way.
pub(crate) fn merge_tokens(php: Vec<Token>, sql: Vec<Token>) -> Vec<Token> {
    if sql.is_empty() {
        return php;
    }
    let mut sql = sql;
    sql.sort_by_key(|token| token.start);
    let mut out: Vec<Token> = php
        .into_iter()
        .filter(|token| {
            let at = sql.partition_point(|other| other.end <= token.start);
            sql.get(at).is_none_or(|other| other.start >= token.end)
        })
        .collect();
    out.extend(sql);
    out.sort_by_key(|token| (token.start, token.end));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(start: u32, end: u32, ty: u32) -> Token {
        Token {
            start,
            end,
            ty,
            modifiers: 0,
        }
    }

    #[test]
    fn the_legend_keeps_php_first_and_adds_what_sql_has() {
        let types = legend_types();
        assert_eq!(
            types[..php_analysis::semantic_tokens::TOKEN_TYPES.len()],
            *php_analysis::semantic_tokens::TOKEN_TYPES
        );
        for name in ["string", "number", "comment", "operator", "type"] {
            assert!(types.contains(&name), "{name}");
        }
        let keyword = TOKEN_TYPES.iter().position(|name| *name == "keyword").expect("keyword") as u32;
        let (ty, _) = token_in_legend(keyword, 0, &types, &legend_modifiers()).expect("in the legend");
        assert_eq!(types[ty as usize], "keyword");
    }

    #[test]
    fn sql_tokens_replace_the_php_tokens_they_cover() {
        let merged = merge_tokens(
            vec![token(0, 4, 1), token(10, 20, 1), token(30, 34, 1)],
            vec![token(12, 15, 2), token(5, 9, 2)],
        );
        let spans: Vec<(u32, u32, u32)> = merged.iter().map(|token| (token.start, token.end, token.ty)).collect();
        assert_eq!(spans, [(0, 4, 1), (5, 9, 2), (12, 15, 2), (30, 34, 1)]);
    }
}
