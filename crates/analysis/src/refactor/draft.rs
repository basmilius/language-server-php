//! Where the edits of a refactor are collected and made fit for the client: applied to the text,
//! laid out by the formatter on the lines they touch, and turned back into the few edits that
//! lead from the old text to the new one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use php_format::FormatOptions;
use php_index::UseKind;
use php_syntax::{TextRange, parse};

use super::diff::hunks;
use super::{Change, Edit, FileChange, FileMove, Focus, RefactorEnv};
use crate::actions::edits::{apply, line_end, line_start};
use crate::actions::imports::remove_import;
use crate::completion::TextEdit;
use crate::inspections::unused::import_clauses;
use crate::inspections::{Cx, InspectionEnv};

/// Brackets around the text that names a new thing, so that the client can start a rename on it
/// once the formatter has moved everything around it.
const FOCUS_OPEN: char = '\u{1}';
const FOCUS_CLOSE: char = '\u{2}';

/// The name a refactor writes, marked as the one a person is expected to change.
pub(crate) fn focus(name: &str) -> String {
    format!("{FOCUS_OPEN}{name}{FOCUS_CLOSE}")
}

/// The edits of a refactor, per file, with the files it moves.
pub(crate) struct Draft<'a> {
    renv: &'a RefactorEnv<'a>,
    files: BTreeMap<PathBuf, Vec<TextEdit>>,
    moves: Vec<FileMove>,
}

impl<'a> Draft<'a> {
    pub(crate) fn new(renv: &'a RefactorEnv<'a>) -> Draft<'a> {
        Draft {
            renv,
            files: BTreeMap::new(),
            moves: Vec::new(),
        }
    }

    pub(crate) fn edit(&mut self, path: &Path, edit: TextEdit) {
        self.files.entry(path.to_path_buf()).or_default().push(edit);
    }

    pub(crate) fn edits(&mut self, path: &Path, edits: impl IntoIterator<Item = TextEdit>) {
        self.files.entry(path.to_path_buf()).or_default().extend(edits);
    }

    /// An edit in the file the refactor was asked in.
    pub(crate) fn here(&mut self, edit: TextEdit) {
        let path = self.renv.path;
        self.edit(path, edit);
    }

    pub(crate) fn here_all(&mut self, edits: impl IntoIterator<Item = TextEdit>) {
        let path = self.renv.path;
        self.edits(path, edits);
    }

    pub(crate) fn move_file(&mut self, from: PathBuf, to: PathBuf) {
        self.moves.push(FileMove { from, to });
    }

    fn text_of(renv: &RefactorEnv<'_>, path: &Path) -> Option<String> {
        if path == renv.path {
            return Some(renv.env.text.to_string());
        }
        renv.sources.text(path)
    }

    /// Applies the edits, formats what they touched and works out what the client has to change.
    pub(crate) fn finish(self) -> Result<Change, String> {
        let Draft {
            renv,
            files: drafted,
            moves,
        } = self;
        let mut files = Vec::new();
        let mut found_focus = None;
        for (path, mut edits) in drafted {
            if edits.is_empty() {
                continue;
            }
            let Some(original) = Draft::text_of(renv, &path) else {
                return Err(format!("{} cannot be read", path.display()));
            };
            edits.sort_by_key(|edit| (edit.start, edit.end));
            edits.dedup();
            check_edits(&original, &edits)?;
            let eol = if original.contains("\r\n") { "\r\n" } else { "\n" };
            if eol == "\r\n" {
                for edit in &mut edits {
                    edit.new_text = to_crlf(&edit.new_text);
                }
            }
            let (written, touched, mark) = write(&original, &edits);
            let mut final_text = written.text;
            let mut mark = mark;
            let mut formatted = false;
            if !touched.is_empty() {
                if let Some((laid_out, moved)) = lay_out(&original, &final_text, &touched, &renv.format, mark) {
                    final_text = laid_out;
                    mark = moved;
                    formatted = true;
                }
            }
            {
                if let Some((tidied, moved)) = tidy_imports(renv, &original, &final_text, mark) {
                    final_text = tidied;
                    mark = moved;
                    formatted = true;
                }
            }
            let list: Vec<Edit> = if formatted {
                hunks(&original, &final_text)
                    .into_iter()
                    .map(|hunk| Edit {
                        start: hunk.old_start as u32,
                        end: hunk.old_end as u32,
                        text: final_text[hunk.new_start..hunk.new_end].to_string(),
                        new_start: hunk.new_start as u32,
                    })
                    .collect()
            } else {
                edits
                    .iter()
                    .zip(&touched)
                    .filter(|(edit, (begin, end))| {
                        original[edit.start as usize..edit.end as usize] != final_text[*begin..*end]
                    })
                    .map(|(edit, &(begin, end))| Edit {
                        start: edit.start,
                        end: edit.end,
                        text: final_text[begin..end].to_string(),
                        new_start: begin as u32,
                    })
                    .collect()
            };
            if let Some((start, end)) = mark {
                found_focus.get_or_insert(Focus {
                    path: path.clone(),
                    start: start as u32,
                    end: end as u32,
                });
            }
            if !list.is_empty() {
                files.push(FileChange { path, edits: list });
            }
        }
        if files.is_empty() && moves.is_empty() {
            return Err("There is nothing to change".to_string());
        }
        Ok(Change {
            files,
            moves,
            focus: found_focus,
        })
    }
}

fn to_crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut previous = '\0';
    for character in text.chars() {
        if character == '\n' && previous != '\r' {
            out.push('\r');
        }
        out.push(character);
        previous = character;
    }
    out
}

fn check_edits(text: &str, edits: &[TextEdit]) -> Result<(), String> {
    let mut last_end = 0;
    for edit in edits {
        let (start, end) = (edit.start as usize, edit.end as usize);
        if start > end || end > text.len() || !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err("The refactor produced an edit outside the file".to_string());
        }
        if start < last_end {
            return Err("The refactor produced edits that overlap".to_string());
        }
        last_end = end;
    }
    Ok(())
}

struct Written {
    text: String,
}

/// The text after the edits, the stretches of it the edits wrote, and where the marked name is.
type Laid = (Written, Vec<(usize, usize)>, Option<(usize, usize)>);

fn write(original: &str, edits: &[TextEdit]) -> Laid {
    let mut text = String::with_capacity(original.len());
    let mut touched = Vec::new();
    let mut mark: Option<(usize, usize)> = None;
    let mut at = 0;
    for edit in edits {
        text.push_str(&original[at..edit.start as usize]);
        let begin = text.len();
        let mut open = None;
        for character in edit.new_text.chars() {
            match character {
                FOCUS_OPEN => open = Some(text.len()),
                FOCUS_CLOSE => {
                    if let (Some(start), None) = (open, mark) {
                        mark = Some((start, text.len()));
                    }
                }
                other => text.push(other),
            }
        }
        touched.push((begin, text.len()));
        at = edit.end as usize;
    }
    text.push_str(&original[at..]);
    (Written { text }, touched, mark)
}

/// Formats the lines the edits wrote. A text the formatter will not read stays as it was written.
fn lay_out(
    original: &str,
    text: &str,
    touched: &[(usize, usize)],
    options: &FormatOptions,
    mark: Option<(usize, usize)>,
) -> Option<(String, Option<(usize, usize)>)> {
    if !parse(original).errors().is_empty() {
        return None;
    }
    let edits = php_format::edits(text, options)?;
    let lines: Vec<(usize, usize)> = touched
        .iter()
        .map(|&(start, end)| {
            (
                line_start(text, start),
                line_end(text, end).saturating_sub(1).max(start),
            )
        })
        .collect();
    let wanted: Vec<php_format::Edit> = edits
        .into_iter()
        .filter(|edit| lines.iter().any(|&(from, to)| (from..=to).contains(&edit.end)))
        .collect();
    if wanted.is_empty() {
        return None;
    }
    let formatted = php_format::apply(text, &wanted);
    let moved = |position: usize| {
        let delta: isize = wanted
            .iter()
            .filter(|edit| edit.end <= position)
            .map(|edit| edit.text.len() as isize - (edit.end - edit.start) as isize)
            .sum();
        (position as isize + delta) as usize
    };
    let mark = mark.map(|(start, end)| (moved(start), moved(end)));
    Some((formatted, mark))
}

/// The text with the edits applied, for tests and checks.
#[allow(dead_code)]
pub(crate) fn applied(text: &str, edits: &[Edit]) -> String {
    let converted: Vec<TextEdit> = edits
        .iter()
        .map(|edit| TextEdit {
            start: edit.start,
            end: edit.end,
            new_text: edit.text.clone(),
        })
        .collect();
    apply(text, &converted)
}

/// The `use` clauses of a text that nothing refers to, by what they import.
fn unused_clauses(renv: &RefactorEnv<'_>, text: &str) -> Vec<(String, UseKind, TextRange)> {
    let tree = parse(text);
    let root = tree.syntax();
    let env = InspectionEnv {
        index: renv.env.index,
        text,
        root: &root,
        settings: renv.env.settings,
        ready: renv.env.ready,
        externals: renv.env.externals,
    };
    let cx = Cx::new(&env);
    import_clauses(&cx)
        .into_iter()
        .filter(|clause| !clause.used)
        .map(|clause| {
            let alone = clause.clause.parent().is_some_and(|parent| parent == clause.statement)
                && clause
                    .statement
                    .children()
                    .filter(|child| child.kind() == php_syntax::SyntaxKind::USE_CLAUSE)
                    .count()
                    == 1;
            let range = if alone {
                clause.statement.text_range()
            } else {
                clause.clause.text_range()
            };
            (clause.full.to_ascii_lowercase(), clause.kind, range)
        })
        .collect()
}

/// Takes out the imports that the change left nothing to use, and only those: what was unused
/// before is not this refactor's business.
fn tidy_imports(
    renv: &RefactorEnv<'_>,
    original: &str,
    text: &str,
    mark: Option<(usize, usize)>,
) -> Option<(String, Option<(usize, usize)>)> {
    if !original.contains("use ") || !parse(original).errors().is_empty() {
        return None;
    }
    let before: Vec<(String, UseKind)> = unused_clauses(renv, original)
        .into_iter()
        .map(|(full, kind, _)| (full, kind))
        .collect();
    let mut text = text.to_string();
    let mut mark = mark;
    let mut changed = false;
    for _ in 0..8 {
        let found = unused_clauses(renv, &text);
        let Some((_, _, range)) = found
            .into_iter()
            .find(|(full, kind, _)| !before.iter().any(|(old, old_kind)| old == full && old_kind == kind))
        else {
            break;
        };
        let tree = parse(&text);
        let Some(edit) = remove_import(&text, &tree.syntax(), range) else {
            break;
        };
        let (from, to) = (edit.start as usize, edit.end as usize);
        let delta = edit.new_text.len() as isize - (to - from) as isize;
        mark = mark.map(|(start, end)| {
            if to <= start {
                ((start as isize + delta) as usize, (end as isize + delta) as usize)
            } else {
                (start, end)
            }
        });
        text.replace_range(from..to, &edit.new_text);
        changed = true;
    }
    changed.then_some((text, mark))
}
