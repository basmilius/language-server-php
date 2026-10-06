//! Code actions: the quick fixes and intentions of `php-analysis`, as LSP code actions with their
//! edits as workspace edits, and the doc block that `/**` and Enter leave behind.

use std::collections::HashMap;

use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, CodeActionResponse, DocumentChanges,
    DocumentOnTypeFormattingParams, OneOf, OptionalVersionedTextDocumentIdentifier, TextDocumentEdit, TextEdit, Uri,
    WorkspaceEdit,
};
use php_analysis::actions::{Action, ActionInput, with_actions};
use php_analysis::inspections::inspect;
use php_syntax::{TextRange, TextSize};
use serde_json::{Value, json};

use crate::convert::{self, Mapper};
use crate::server::Server;

/// Whether a kind is one the client asked for: the kind itself or one below it.
pub(crate) fn wanted(kind: &str, only: Option<&[CodeActionKind]>) -> bool {
    let Some(only) = only else {
        return true;
    };
    only.iter().any(|requested| {
        let requested = requested.as_str();
        kind == requested || kind.starts_with(&format!("{requested}."))
    })
}

fn lsp_edits(mapper: &Mapper, edits: &[php_analysis::completion::TextEdit]) -> Vec<TextEdit> {
    edits
        .iter()
        .map(|edit| TextEdit {
            range: mapper.range(TextRange::new(TextSize::from(edit.start), TextSize::from(edit.end))),
            new_text: edit.new_text.clone(),
        })
        .collect()
}

fn workspace_edit(
    uri: &Uri,
    version: i32,
    document_changes: bool,
    mapper: &Mapper,
    edits: &[php_analysis::completion::TextEdit],
) -> WorkspaceEdit {
    let edits = lsp_edits(mapper, edits);
    if document_changes {
        return WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: Some(version),
                },
                edits: edits.into_iter().map(OneOf::Left).collect(),
            }])),
            change_annotations: None,
        };
    }
    WorkspaceEdit {
        changes: Some(HashMap::from([(uri.clone(), edits)])),
        document_changes: None,
        change_annotations: None,
    }
}

pub(crate) fn selection(mapper: &Mapper, range: lsp_types::Range) -> TextRange {
    let (start, end) = (mapper.offset(range.start), mapper.offset(range.end));
    TextRange::new(start.min(end), start.max(end))
}

impl Server {
    pub(crate) fn code_action(&mut self, params: CodeActionParams) -> Option<Vec<Value>> {
        let uri = params.text_document.uri.clone();
        let resolve = self.code_action_resolve;
        let document_changes = self.document_changes;
        let version = self.documents.get(&uri)?.version;
        let only = params.context.only.clone();
        let refactors = self.refactor_actions(&params);
        let fixes = self.inspect_document(&uri, |env, mapper, _| {
            let range = selection(mapper, params.range);
            let findings = inspect(env);
            let input = ActionInput {
                env,
                range,
                findings: &findings,
            };
            with_actions(&input, |actions| {
                actions
                    .iter()
                    .filter(|action| wanted(action.kind.name(), only.as_deref()))
                    .map(|action| {
                        let defer = resolve && action.expensive;
                        CodeActionOrCommand::CodeAction(CodeAction {
                            title: action.title.clone(),
                            kind: Some(CodeActionKind::from(action.kind.name().to_string())),
                            diagnostics: action
                                .finding
                                .and_then(|position| findings.get(position))
                                .map(|finding| vec![convert::diagnostic(mapper, &finding.diagnostic)]),
                            edit: (!defer)
                                .then(|| workspace_edit(&uri, version, document_changes, mapper, &action.edits())),
                            is_preferred: action.preferred.then_some(true),
                            data: defer.then(|| {
                                json!({
                                    "uri": uri.as_str(),
                                    "version": version,
                                    "start": u32::from(range.start()),
                                    "end": u32::from(range.end()),
                                    "id": action.id,
                                })
                            }),
                            ..CodeAction::default()
                        })
                    })
                    .collect::<CodeActionResponse>()
            })
        })?;
        let mut actions: Vec<Value> = fixes
            .into_iter()
            .filter_map(|action| serde_json::to_value(action).ok())
            .collect();
        actions.extend(refactors);
        Some(actions)
    }

    /// Works out the edits of an action that was sent without them.
    pub(crate) fn resolve_code_action(&mut self, action: Value) -> Result<Option<Value>, String> {
        if action.pointer("/data/refactor").and_then(Value::as_bool) == Some(true) {
            return self.resolve_refactor(action);
        }
        let Ok(action) = serde_json::from_value::<CodeAction>(action) else {
            return Ok(None);
        };
        Ok(self
            .resolve_fix(action)
            .and_then(|resolved| serde_json::to_value(resolved).ok()))
    }

    fn resolve_fix(&mut self, mut action: CodeAction) -> Option<CodeAction> {
        let data = action.data.clone()?;
        let uri: Uri = data.get("uri")?.as_str()?.parse().ok()?;
        let version = i32::try_from(data.get("version")?.as_i64()?).ok()?;
        let id = data.get("id")?.as_str()?.to_string();
        let (start, end) = (data.get("start")?.as_u64()?, data.get("end")?.as_u64()?);
        let document_changes = self.document_changes;
        if self.documents.get(&uri)?.version != version {
            return Some(action);
        }
        let range = TextRange::new(
            TextSize::from(u32::try_from(start).ok()?),
            TextSize::from(u32::try_from(end).ok()?),
        );
        let edit = self.inspect_document(&uri, |env, mapper, _| {
            let findings = inspect(env);
            let input = ActionInput {
                env,
                range,
                findings: &findings,
            };
            with_actions(&input, |actions| {
                let found: Option<&Action<'_>> = actions.iter().find(|candidate| candidate.id == id);
                found.map(|found| workspace_edit(&uri, version, document_changes, mapper, &found.edits()))
            })
        })??;
        action.edit = Some(edit);
        action.data = None::<Value>;
        Some(action)
    }

    /// The edits a typed character asks for: the doc block after `/**` and Enter, else the layout of
    /// the line the character ended.
    pub(crate) fn on_type_formatting(&mut self, params: DocumentOnTypeFormattingParams) -> Option<Vec<TextEdit>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let position = params.text_document_position.position;
        let stub = self.inspect_document(&uri, |env, mapper, _| {
            let offset = u32::from(mapper.offset(position));
            if params.ch == "\n" {
                return php_analysis::actions::doc_stub_at(env, offset).map(|stub| lsp_edits(mapper, &[stub]));
            }
            None
        })?;
        match stub {
            Some(edits) => Some(edits),
            None => self.typed_formatting(&params).or_else(|| Some(Vec::new())),
        }
    }
}
