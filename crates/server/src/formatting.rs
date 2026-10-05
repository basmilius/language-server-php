//! `textDocument/formatting`, `rangeFormatting` and the formatting part of `onTypeFormatting`.

use lsp_types::{
    DocumentFormattingParams, DocumentOnTypeFormattingParams, DocumentRangeFormattingParams, FormattingOptions,
    TextEdit, Uri,
};
use php_format::{Edit, FormatOptions, Indent};
use php_syntax::{TextRange, TextSize};

use crate::convert::Mapper;
use crate::server::Server;

/// What the client asks for is indentation; the rest comes from its settings.
fn indent_of(options: &FormattingOptions) -> Indent {
    if options.insert_spaces {
        Indent::Spaces(usize::try_from(options.tab_size).unwrap_or(4).clamp(1, 16))
    } else {
        Indent::Tab
    }
}

impl Server<'_> {
    /// The options with what the `.editorconfig` of the file's project asks for, unless the
    /// settings turn that off. The client's indentation is only what the editor happens to be
    /// set to, so the project's own file goes before it.
    pub(crate) fn with_editorconfig(&self, uri: &Uri, options: FormatOptions) -> FormatOptions {
        let wanted = self
            .settings
            .format
            .as_ref()
            .and_then(|settings| settings.editorconfig)
            .unwrap_or(true);
        let Some(path) = crate::paths::uri_to_path(uri).filter(|_| wanted) else {
            return options;
        };
        let config = php_format::EditorConfig::for_path(&path, &|file| std::fs::read_to_string(file).ok());
        options.with_editorconfig(&config)
    }

    fn format_options_for(&self, uri: &Uri, asked: &FormattingOptions) -> FormatOptions {
        let mut options = FormatOptions {
            indent: indent_of(asked),
            ..FormatOptions::default()
        };
        options = self.with_editorconfig(uri, options);
        if let Some(settings) = &self.settings.format {
            options = settings.apply(options);
        }
        if let Some(settings) = self.documents.get(uri).and_then(|document| document.format.as_ref()) {
            options = settings.apply(options);
        }
        options
    }

    /// Runs the formatter over an open document and turns what it changes into edits. `None` when the
    /// text is left as it is, which a client shows as nothing to do.
    fn formatted_edits(
        &self,
        uri: &Uri,
        run: impl FnOnce(&str, &FormatOptions) -> Option<Vec<Edit>>,
        asked: &FormattingOptions,
    ) -> Option<Vec<TextEdit>> {
        let options = self.format_options_for(uri, asked);
        let document = self.documents.get(uri)?;
        let edits = run(&document.text, &options)?;
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding: self.encoding,
        };
        Some(
            edits
                .into_iter()
                .map(|edit| TextEdit {
                    range: mapper.range(TextRange::new(
                        TextSize::from(edit.start as u32),
                        TextSize::from(edit.end as u32),
                    )),
                    new_text: edit.text,
                })
                .collect(),
        )
    }

    pub(crate) fn formatting(&mut self, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
        self.formatted_edits(&params.text_document.uri, php_format::edits, &params.options)
    }

    pub(crate) fn range_formatting(&mut self, params: DocumentRangeFormattingParams) -> Option<Vec<TextEdit>> {
        let uri = params.text_document.uri;
        let document = self.documents.get(&uri)?;
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding: self.encoding,
        };
        let start = usize::from(mapper.offset(params.range.start));
        let end = usize::from(mapper.offset(params.range.end));
        self.formatted_edits(
            &uri,
            |text, options| php_format::range_edits(text, start, end, options),
            &params.options,
        )
    }

    /// The edits for a typed `}`, `;` or newline, which is `None` for any other character.
    pub(crate) fn typed_formatting(&mut self, params: &DocumentOnTypeFormattingParams) -> Option<Vec<TextEdit>> {
        let typed = params
            .ch
            .chars()
            .next()
            .filter(|typed| matches!(typed, '}' | ';' | '\n'))?;
        let uri = &params.text_document_position.text_document.uri;
        let document = self.documents.get(uri)?;
        let mapper = Mapper {
            text: &document.text,
            index: &document.index,
            encoding: self.encoding,
        };
        let offset = usize::from(mapper.offset(params.text_document_position.position));
        self.formatted_edits(
            uri,
            |text, options| php_format::on_type_edits(text, offset, typed, options),
            &params.options,
        )
    }
}
