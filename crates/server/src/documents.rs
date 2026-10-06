use std::collections::HashMap;

use lsp_types::{TextDocumentContentChangeEvent, Uri};
use php_analysis::inspections::InspectionSettings;

use crate::config::FormatSettings;
use php_analysis::{LineCol, LineIndex, PositionEncoding};
use php_syntax::{Parse, PhpVersion, parse};

/// An open document: its text, the tree of that text and the language level it is read at.
pub struct Document {
    pub version: i32,
    pub text: String,
    pub index: LineIndex,
    /// The language level of this document alone, when the client gave one.
    pub level: Option<PhpVersion>,
    /// The inspection settings of this document alone, when the client gave some.
    pub inspections: Option<InspectionSettings>,
    /// The formatting settings of this document alone, when the client gave some.
    pub format: Option<FormatSettings>,
    /// Whether a search from this document reads the installed packages, when the client said.
    pub usages_packages: Option<bool>,
    parsed: Option<Parse>,
    /// The version whose declarations the index holds.
    pub indexed_version: Option<i32>,
    /// A Blade template: only the names it holds and the PHP in it are read.
    pub blade: bool,
    /// A Twig template, which is no PHP at all.
    pub twig: bool,
    /// A YAML file, of which only a Symfony project's configuration is read.
    pub yaml: bool,
}

impl Document {
    pub fn new(version: i32, text: String) -> Document {
        let index = LineIndex::new(&text);
        Document {
            version,
            text,
            index,
            level: None,
            inspections: None,
            format: None,
            usages_packages: None,
            parsed: None,
            indexed_version: None,
            blade: false,
            twig: false,
            yaml: false,
        }
    }

    /// The tree of the current text. The whole file is parsed again after a change, which takes
    /// well under a millisecond for a typical file, so there is nothing to patch up.
    pub fn parse(&mut self) -> &Parse {
        self.parsed.get_or_insert_with(|| parse(&self.text))
    }

    /// The tree of the text, when it was parsed since the last change.
    pub fn cached(&self) -> Option<&Parse> {
        self.parsed.as_ref()
    }

    pub fn apply_changes(
        &mut self,
        version: i32,
        changes: &[TextDocumentContentChangeEvent],
        encoding: PositionEncoding,
    ) {
        for change in changes {
            match change.range {
                None => self.text.clone_from(&change.text),
                Some(range) => {
                    let start = self.index.offset(
                        &self.text,
                        LineCol {
                            line: range.start.line,
                            col: range.start.character,
                        },
                        encoding,
                    );
                    let end = self.index.offset(
                        &self.text,
                        LineCol {
                            line: range.end.line,
                            col: range.end.character,
                        },
                        encoding,
                    );
                    let (start, end) = if end < start { (end, start) } else { (start, end) };
                    self.text
                        .replace_range(usize::from(start)..usize::from(end), &change.text);
                }
            }
            self.index = LineIndex::new(&self.text);
        }
        self.version = version;
        self.parsed = None;
    }
}

#[derive(Default)]
pub struct Documents {
    open: HashMap<Uri, Document>,
}

impl Documents {
    pub fn open(&mut self, uri: Uri, version: i32, text: String) {
        self.open.insert(uri, Document::new(version, text));
    }

    pub fn close(&mut self, uri: &Uri) -> Option<Document> {
        self.open.remove(uri)
    }

    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.open.get(uri)
    }

    pub fn get_mut(&mut self, uri: &Uri) -> Option<&mut Document> {
        self.open.get_mut(uri)
    }

    /// The text of every open document by path, for searches that must see unsaved changes.
    pub fn texts(&self) -> std::collections::HashMap<std::path::PathBuf, String> {
        self.open
            .iter()
            .filter_map(|(uri, document)| Some((crate::paths::uri_to_path(uri)?, document.text.clone())))
            .collect()
    }

    pub fn uris(&self) -> Vec<Uri> {
        self.open.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    fn change(start: (u32, u32), end: (u32, u32), text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(Range::new(Position::new(start.0, start.1), Position::new(end.0, end.1))),
            range_length: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn applies_incremental_changes_in_order() {
        let mut document = Document::new(1, "<?php\n$a = 1;\n$b = 2;\n".to_string());
        document.apply_changes(
            2,
            &[change((1, 5), (1, 6), "42"), change((2, 0), (2, 0), "// x\n")],
            PositionEncoding::Utf16,
        );
        assert_eq!(document.text, "<?php\n$a = 42;\n// x\n$b = 2;\n");
        assert_eq!(document.version, 2);
        assert!(document.parse().errors().is_empty());
    }

    #[test]
    fn counts_utf16_columns_after_astral_characters() {
        let mut document = Document::new(1, "<?php\n$a = '\u{1f600}x';\n".to_string());
        document.apply_changes(2, &[change((1, 8), (1, 9), "y")], PositionEncoding::Utf16);
        assert_eq!(document.text, "<?php\n$a = '\u{1f600}y';\n");
        let mut utf8 = Document::new(1, "<?php\n$a = '\u{1f600}x';\n".to_string());
        utf8.apply_changes(2, &[change((1, 10), (1, 11), "y")], PositionEncoding::Utf8);
        assert_eq!(utf8.text, document.text);
    }

    #[test]
    fn a_change_without_a_range_replaces_the_text() {
        let mut document = Document::new(1, "<?php 1;".to_string());
        let whole = TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "<?php 2;".to_string(),
        };
        document.apply_changes(2, &[whole], PositionEncoding::Utf16);
        assert_eq!(document.text, "<?php 2;");
    }
}
