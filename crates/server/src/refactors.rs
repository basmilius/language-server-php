//! Refactors as code actions, and the edits that follow a file when it is renamed or moved.

use std::path::{Path, PathBuf};

use lsp_types::{CodeActionParams, Range, RenameFilesParams, Uri};
use php_analysis::LineIndex;
use php_analysis::inspections::{Externals, InspectionEnv};
use php_analysis::refactor::{Change, FileChange, RefactorEnv, move_files, with_refactors};
use php_format::{FormatOptions, Indent};
use php_syntax::{TextRange, TextSize};
use serde_json::{Value, json};

use crate::actions::{selection, wanted};
use crate::convert::Mapper;
use crate::paths::{path_to_uri, uri_to_path};
use crate::server::Server;
use crate::usages::ProjectSources;

/// The command a client runs after applying a refactor that wrote a name, to start a rename on it.
pub(crate) const RENAME_COMMAND: &str = "php.rename";

/// A refactor as it is sent: the work of the edits already done, or left for `codeAction/resolve`.
struct Offered {
    title: String,
    kind: &'static str,
    id: String,
    change: Option<Change>,
}

/// The indentation a file already uses, which is what code written into it should follow.
fn indent_of_text(text: &str) -> Option<Indent> {
    let line = text
        .lines()
        .find(|line| line.starts_with(' ') || line.starts_with('\t'))?;
    if line.starts_with('\t') {
        return Some(Indent::Tab);
    }
    let width = line.len() - line.trim_start_matches(' ').len();
    Some(Indent::Spaces(if width % 4 == 0 { 4 } else { width.clamp(1, 8) }))
}

impl Server<'_> {
    /// The options for the lines a refactor writes: what the file uses, then what the settings say.
    fn refactor_format_options(&self, uri: &Uri) -> FormatOptions {
        let mut options = FormatOptions::default();
        if let Some(document) = self.documents.get(uri) {
            if let Some(indent) = indent_of_text(&document.text) {
                options.indent = indent;
            }
        }
        options = self.with_editorconfig(uri, options);
        if let Some(settings) = &self.settings.format {
            options = settings.apply(options);
        }
        if let Some(settings) = self.documents.get(uri).and_then(|document| document.format.as_ref()) {
            options = settings.apply(options);
        }
        options
    }

    /// The text of a file as the client sees it: the open document, else the disk.
    pub(crate) fn text_of_file(&self, path: &Path) -> Option<String> {
        if let Some(uri) = path_to_uri(path) {
            if let Some(document) = self.documents.get(&uri) {
                return Some(document.text.clone());
            }
        }
        std::fs::read(path)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The refactors at a range of a document, as LSP code actions.
    pub(crate) fn refactor_actions(&mut self, params: &CodeActionParams) -> Vec<Value> {
        let uri = params.text_document.uri.clone();
        let only = params.context.only.clone();
        let Some(version) = self.documents.get(&uri).map(|document| document.version) else {
            return Vec::new();
        };
        let defer = self.code_action_resolve;
        let asked = params.range;
        let offered = self.with_refactor_env(
            &uri,
            !defer,
            |mapper| selection(mapper, asked),
            |refactors| {
                refactors
                    .iter()
                    .filter(|refactor| wanted(refactor.kind.name(), only.as_deref()))
                    .filter_map(|refactor| {
                        let change = if defer && refactor.expensive {
                            None
                        } else {
                            Some(refactor.run().ok()?)
                        };
                        Some(Offered {
                            title: refactor.title.clone(),
                            kind: refactor.kind.name(),
                            id: refactor.id.clone(),
                            change,
                        })
                    })
                    .collect::<Vec<Offered>>()
            },
        );
        let Some(offered) = offered else {
            return Vec::new();
        };
        let range = self.selection_offsets(&uri, params.range);
        offered
            .into_iter()
            .filter_map(|item| {
                let mut action = json!({ "title": item.title, "kind": item.kind });
                match item.change {
                    Some(change) => {
                        let (edit, command) = self.change_to_edit(&change, &uri).ok()?;
                        action["edit"] = edit;
                        if let Some(command) = command {
                            action["command"] = command;
                        }
                    }
                    None => {
                        let (start, end) = range?;
                        action["data"] = json!({
                            "uri": uri.as_str(),
                            "version": version,
                            "start": start,
                            "end": end,
                            "id": item.id,
                            "refactor": true,
                        });
                    }
                }
                Some(action)
            })
            .collect()
    }

    fn selection_offsets(&self, uri: &Uri, range: Range) -> Option<(u32, u32)> {
        let document = self.documents.get(uri)?;
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding: self.encoding,
        };
        let range = selection(&mapper, range);
        Some((u32::from(range.start()), u32::from(range.end())))
    }

    /// Works out the edits of a refactor that was sent without them.
    pub(crate) fn resolve_refactor(&mut self, mut action: Value) -> Result<Option<Value>, String> {
        let data = action.get("data").cloned().ok_or("The action has nothing to resolve")?;
        let text = |key: &str| data.get(key).and_then(Value::as_str);
        let number = |key: &str| {
            data.get(key)
                .and_then(Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
        };
        let uri: Uri = text("uri")
            .and_then(|uri| uri.parse().ok())
            .ok_or("The action has no document")?;
        let id = text("id").ok_or("The action has no id")?.to_string();
        let (Some(start), Some(end)) = (number("start"), number("end")) else {
            return Err("The action has no range".to_string());
        };
        let version = data.get("version").and_then(Value::as_i64);
        if self.documents.get(&uri).map(|document| i64::from(document.version)) != version {
            return Ok(Some(action));
        }
        let range = TextRange::new(TextSize::from(start), TextSize::from(end));
        let change = self
            .with_refactor_env(
                &uri,
                true,
                |_| range,
                |refactors| {
                    refactors
                        .iter()
                        .find(|refactor| refactor.id == id)
                        .map(|refactor| refactor.run())
                },
            )
            .flatten()
            .ok_or("This refactor does not apply here any more")??;
        let (edit, command) = self.change_to_edit(&change, &uri)?;
        action["edit"] = edit;
        if let Some(command) = command {
            action["command"] = command;
        }
        if let Some(object) = action.as_object_mut() {
            object.remove("data");
        }
        Ok(Some(action))
    }

    /// Runs something over the refactors at a range. The words of the project are only read when
    /// something is going to run that searches it, which listing the refactors of a client that
    /// resolves them late does not.
    fn with_refactor_env<R>(
        &mut self,
        uri: &Uri,
        needs_words: bool,
        pick: impl FnOnce(&Mapper<'_>) -> TextRange,
        run: impl FnOnce(Vec<php_analysis::refactor::Refactor<'_>>) -> R,
    ) -> Option<R> {
        let path = uri_to_path(uri)?;
        if needs_words {
            self.ensure_words(&path);
        }
        let open = self.documents.texts();
        let format = self.refactor_format_options(uri);
        self.inspect_document_in(uri, |env, mapper, _, project| {
            let sources = ProjectSources::new(open, &project.words);
            let renv = RefactorEnv {
                env,
                path: &path,
                sources: &sources,
                composer: project.composer.as_ref(),
                format,
            };
            let range = pick(mapper);
            with_refactors(&renv, range, run)
        })
    }

    /// The edit of a change as a workspace edit, and the command that starts a rename of the name
    /// the change wrote when the client cannot place the cursor itself.
    pub(crate) fn change_to_edit(&self, change: &Change, asked_in: &Uri) -> Result<(Value, Option<Value>), String> {
        let mut documents: Vec<Value> = Vec::new();
        let mut changes = serde_json::Map::new();
        let mut command = None;
        let snippets = self.snippet_edits && self.document_changes;
        if !change.creates.is_empty() {
            if !(self.create_files && self.document_changes) {
                return Err("The client cannot make files, so this refactor cannot be applied".to_string());
            }
            for created in &change.creates {
                let uri = path_to_uri(&created.path).ok_or("A file to make has no URI")?;
                documents.push(json!({ "kind": "create", "uri": uri.as_str(), "options": { "overwrite": false, "ignoreIfExists": false } }));
                documents.push(json!({
                    "textDocument": { "uri": uri.as_str(), "version": null },
                    "edits": [{
                        "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                        "newText": created.text,
                    }],
                }));
            }
        }
        for file in &change.files {
            let (uri, edits, rename_at) = self.file_edits(file, change, snippets)?;
            if self.document_changes {
                let version = self.documents.get(&uri).map(|document| document.version);
                documents.push(json!({
                    "textDocument": { "uri": uri.as_str(), "version": version },
                    "edits": edits,
                }));
            } else {
                changes.insert(uri.as_str().to_string(), Value::Array(edits));
            }
            if let (Some(position), false) = (rename_at, snippets) {
                command = Some(json!({
                    "title": "Rename",
                    "command": RENAME_COMMAND,
                    "arguments": [{ "textDocument": { "uri": uri.as_str() }, "position": position }],
                }));
            }
        }
        if !change.moves.is_empty() {
            if !(self.rename_files && self.document_changes) {
                return Err("The client cannot move files, so this refactor cannot be applied".to_string());
            }
            for moved in &change.moves {
                let (Some(old_uri), Some(new_uri)) = (path_to_uri(&moved.from), path_to_uri(&moved.to)) else {
                    return Err("A file to move has no URI".to_string());
                };
                documents.push(json!({ "kind": "rename", "oldUri": old_uri.as_str(), "newUri": new_uri.as_str() }));
            }
        }
        let _ = asked_in;
        let edit = if self.document_changes {
            json!({ "documentChanges": documents })
        } else {
            json!({ "changes": Value::Object(changes) })
        };
        Ok((edit, command))
    }

    /// The edits of one file in LSP shapes, and where the rename of the written name starts.
    fn file_edits(
        &self,
        file: &FileChange,
        change: &Change,
        snippets: bool,
    ) -> Result<(Uri, Vec<Value>, Option<Value>), String> {
        let uri = path_to_uri(&file.path).ok_or("A file has no URI")?;
        let text = self
            .text_of_file(&file.path)
            .ok_or_else(|| format!("{} cannot be read", file.path.display()))?;
        let index = LineIndex::new(&text);
        let mapper = Mapper {
            text: &text,
            index: &index,
            encoding: self.encoding,
        };
        let focus = change.focus.as_ref().filter(|focus| focus.path == file.path);
        let mut rename_at = None;
        if let Some(focus) = focus {
            let after = apply_edits(&text, file);
            let after_index = LineIndex::new(&after);
            let after_mapper = Mapper {
                text: &after,
                index: &after_index,
                encoding: self.encoding,
            };
            let position = after_mapper.position(TextSize::from(focus.start));
            rename_at = Some(json!({ "line": position.line, "character": position.character }));
        }
        let edits = file
            .edits
            .iter()
            .map(|edit| {
                let range = mapper.range(TextRange::new(TextSize::from(edit.start), TextSize::from(edit.end)));
                let holds_focus = focus.is_some_and(|focus| {
                    edit.new_start <= focus.start && focus.end <= edit.new_start + edit.text.len() as u32
                });
                match (focus, holds_focus && snippets) {
                    (Some(focus), true) => json!({
                        "range": range,
                        "snippet": { "kind": "snippet", "value": snippet_of(edit, focus.start, focus.end) },
                    }),
                    _ => json!({ "range": range, "newText": edit.text }),
                }
            })
            .collect();
        Ok((uri, edits, rename_at))
    }
}

fn apply_edits(text: &str, file: &FileChange) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for edit in &file.edits {
        out.push_str(&text[at..edit.start as usize]);
        out.push_str(&edit.text);
        at = edit.end as usize;
    }
    out.push_str(&text[at..]);
    out
}

/// The text of an edit as snippet syntax with the written name as the first placeholder, and
/// everything else escaped so that `$name` stays `$name`.
fn snippet_of(edit: &php_analysis::refactor::Edit, focus_start: u32, focus_end: u32) -> String {
    let escape = |text: &str| text.replace('\\', "\\\\").replace('$', "\\$").replace('}', "\\}");
    let from = (focus_start - edit.new_start) as usize;
    let to = (focus_end - edit.new_start) as usize;
    format!(
        "{}${{1:{}}}{}",
        escape(&edit.text[..from]),
        escape(&edit.text[from..to]),
        escape(&edit.text[to..])
    )
}

impl Server<'_> {
    /// The edits that follow files being renamed: the namespace and the name of the class in each,
    /// and every reference to it. A file that never followed its Composer map is left as it is.
    pub(crate) fn will_rename_files(&mut self, params: RenameFilesParams) -> Result<Option<Value>, String> {
        let mut pairs: Vec<(PathBuf, PathBuf)> = Vec::new();
        for file in &params.files {
            let (Ok(old_uri), Ok(new_uri)) = (file.old_uri.parse::<Uri>(), file.new_uri.parse::<Uri>()) else {
                continue;
            };
            let (Some(old), Some(new)) = (uri_to_path(&old_uri), uri_to_path(&new_uri)) else {
                continue;
            };
            pairs.extend(self.paths_moved(&old, &new));
        }
        let Some((first, _)) = pairs.first().cloned() else {
            return Ok(None);
        };
        self.ensure_words(&first);
        let open = self.documents.texts();
        let Some(text) = open.get(&first).cloned().or_else(|| {
            std::fs::read(&first)
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        }) else {
            return Ok(None);
        };
        let format = FormatOptions {
            indent: indent_of_text(&text).unwrap_or(Indent::Spaces(4)),
            ..FormatOptions::default()
        };
        let uri = path_to_uri(&first).ok_or("The file has no URI")?;
        let format = self.with_editorconfig(&uri, format);
        let format = match &self.settings.format {
            Some(settings) => settings.apply(format),
            None => format,
        };
        let settings = self.settings.inspections.clone().unwrap_or_default();
        let tree = php_syntax::parse(&text);
        let root = tree.syntax();
        self.sync_symbols(&uri);
        let names = &self.workspace.stub_names;
        let project = self.workspace.project_for(&first);
        let has_class = |name: &str| names.has_class(name);
        let has_function = |name: &str| names.has_function(name);
        let has_constant = |name: &str| names.has_constant(name);
        let externals = Externals {
            class: &has_class,
            function: &has_function,
            constant: &has_constant,
        };
        let env = InspectionEnv {
            index: &project.index,
            text: &text,
            root: &root,
            settings: &settings,
            ready: false,
            externals: &externals,
        };
        let sources = ProjectSources::new(open, &project.words);
        let renv = RefactorEnv {
            env: &env,
            path: &first,
            sources: &sources,
            composer: project.composer.as_ref(),
            format,
        };
        let change = move_files(&renv, &pairs, false)?;
        let Some(change) = change else {
            return Ok(None);
        };
        let (edit, _) = self.change_to_edit(&change, &uri)?;
        Ok(Some(edit))
    }

    /// The files a rename of a path moves: the file itself, or every PHP file of the project below a folder.
    fn paths_moved(&self, old: &Path, new: &Path) -> Vec<(PathBuf, PathBuf)> {
        if !old.is_dir() {
            return vec![(old.to_path_buf(), new.to_path_buf())];
        }
        let project = self.workspace.project_for(old);
        let mut found: Vec<(PathBuf, PathBuf)> = project
            .index
            .files()
            .filter(|file| file.origin == php_index::Origin::Project)
            .filter_map(|file| {
                let relative = file.path.strip_prefix(old).ok()?;
                Some((file.path.clone(), new.join(relative)))
            })
            .collect();
        found.sort();
        found
    }
}
