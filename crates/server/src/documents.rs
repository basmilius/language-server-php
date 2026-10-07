use php_analysis::inspections::InspectionSettings;
use php_syntax::{Parse, PhpVersion, parse};

use crate::config::FormatSettings;

/// An open document: its text, the tree of that text and what PHP reads it with.
pub type Document = lsc_server::Document<Parse, DocumentState>;

pub type Documents = lsc_server::Documents<Parse, DocumentState>;

/// What the server keeps about an open document besides its text.
#[derive(Default)]
pub struct DocumentState {
    /// The language level of this document alone, when the client gave one.
    pub level: Option<PhpVersion>,
    /// The inspection settings of this document alone, when the client gave some.
    pub inspections: Option<InspectionSettings>,
    /// The formatting settings of this document alone, when the client gave some.
    pub format: Option<FormatSettings>,
    /// Whether a search from this document reads the installed packages, when the client said.
    pub usages_packages: Option<bool>,
    /// The version whose declarations the index holds.
    pub indexed_version: Option<i32>,
    /// A Blade template: only the names it holds and the PHP in it are read.
    pub blade: bool,
    /// A Twig template, which is no PHP at all.
    pub twig: bool,
    /// A YAML file, of which only a Symfony project's configuration is read.
    pub yaml: bool,
    /// The `sql` setting of this document alone, when the client gave one.
    pub sql: Option<serde_json::Value>,
}

pub trait ParseDocument {
    /// The PHP tree of the current text.
    fn parse(&mut self) -> &Parse;
}

impl ParseDocument for Document {
    fn parse(&mut self) -> &Parse {
        self.parse_with(parse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range, TextDocumentContentChangeEvent};
    use php_analysis::PositionEncoding;

    #[test]
    fn parses_the_text_after_incremental_changes() {
        let mut document = Document::new(1, "<?php\n$a = 1;\n$b = 2;\n".to_string());
        document.apply_changes(
            2,
            &[TextDocumentContentChangeEvent {
                range: Some(Range::new(Position::new(1, 5), Position::new(1, 6))),
                range_length: None,
                text: "42".to_string(),
            }],
            PositionEncoding::Utf16,
        );
        assert_eq!(document.text, "<?php\n$a = 42;\n$b = 2;\n");
        assert!(document.parse().errors().is_empty());
        assert!(document.cached().is_some());
    }
}
