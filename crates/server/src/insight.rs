//! Requests that read one document against the index: signature help, semantic tokens and inlay
//! hints.

use lsc_server::paths::uri_to_path;
use lsp_types::{
    Documentation, InlayHint, InlayHintKind, InlayHintLabel, InlayHintParams, MarkupContent, MarkupKind,
    ParameterInformation, ParameterLabel, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend, SemanticTokensParams, SemanticTokensRangeParams, SemanticTokensRangeResult,
    SemanticTokensResult, SignatureHelp, SignatureHelpParams, SignatureInformation,
};
use php_analysis::inlay_hints::{HintKind, HintOptions, inlay_hints};
use php_analysis::semantic_tokens::{TOKEN_MODIFIERS, TOKEN_TYPES, semantic_tokens};
use php_analysis::signature::signature_help;
use php_syntax::TextSize;

use crate::documents::ParseDocument;
use crate::server::Server;

/// The token types and modifiers a client is told to expect, in the order the analysis numbers them.
pub(crate) fn semantic_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.iter().map(|name| SemanticTokenType::new(name)).collect(),
        token_modifiers: TOKEN_MODIFIERS
            .iter()
            .map(|name| SemanticTokenModifier::new(name))
            .collect(),
    }
}

impl Server {
    pub(crate) fn inlay_hints(&mut self, params: InlayHintParams) -> Option<Vec<InlayHint>> {
        let uri = params.text_document.uri;
        let path = uri_to_path(&uri);
        self.sync_symbols(&uri);
        let options = HintOptions {
            parameter_names: self.settings.hint_parameter_names.unwrap_or(true),
            closure_types: self.settings.hint_closure_types.unwrap_or(true),
            property_types: self.settings.hint_property_types.unwrap_or(true),
        };
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let root = document.parse().syntax();
        let mapper = document.mapper(encoding);
        let range = mapper.text_range(params.range);
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let hints = inlay_hints(&project.index, &root, Some(range), options);
        Some(
            hints
                .into_iter()
                .map(|hint| {
                    let position = mapper.position(TextSize::from(hint.offset));
                    InlayHint {
                        position,
                        label: InlayHintLabel::String(hint.label),
                        kind: Some(match hint.kind {
                            HintKind::Parameter => InlayHintKind::PARAMETER,
                            HintKind::Type => InlayHintKind::TYPE,
                        }),
                        text_edits: hint.insert.map(|text| {
                            vec![lsp_types::TextEdit {
                                range: lsp_types::Range::new(position, position),
                                new_text: text,
                            }]
                        }),
                        tooltip: None,
                        padding_left: Some(false),
                        padding_right: Some(true),
                        data: None,
                    }
                })
                .collect(),
        )
    }

    /// The tokens of a document, or of a range of it, in the delta encoding LSP uses.
    fn tokens_of(&mut self, uri: &lsp_types::Uri, range: Option<lsp_types::Range>) -> Option<Vec<SemanticToken>> {
        let path = uri_to_path(uri);
        self.sync_symbols(uri);
        let encoding = self.encoding;
        let document = self.documents.get_mut(uri)?;
        let root = document.parse().syntax();
        let mapper = document.mapper(encoding);
        let range = range.map(|range| mapper.text_range(range));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let tokens = semantic_tokens(&project.index, &root, range);
        let mut data = Vec::with_capacity(tokens.len());
        let (mut previous_line, mut previous_start) = (0, 0);
        for token in tokens {
            let start = mapper.position(TextSize::from(token.start));
            let end = mapper.position(TextSize::from(token.end));
            if end.line != start.line {
                continue;
            }
            let delta_line = start.line - previous_line;
            let delta_start = if delta_line == 0 {
                start.character - previous_start
            } else {
                start.character
            };
            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: end.character - start.character,
                token_type: token.ty,
                token_modifiers_bitset: token.modifiers,
            });
            previous_line = start.line;
            previous_start = start.character;
        }
        Some(data)
    }

    pub(crate) fn semantic_tokens_full(&mut self, params: SemanticTokensParams) -> Option<SemanticTokensResult> {
        let data = self.tokens_of(&params.text_document.uri, None)?;
        Some(SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data }))
    }

    pub(crate) fn semantic_tokens_range(
        &mut self,
        params: SemanticTokensRangeParams,
    ) -> Option<SemanticTokensRangeResult> {
        let data = self.tokens_of(&params.text_document.uri, Some(params.range))?;
        Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        }))
    }

    pub(crate) fn signature_help(&mut self, params: SignatureHelpParams) -> Option<SignatureHelp> {
        let position = params.text_document_position_params;
        let uri = position.text_document.uri;
        let path = uri_to_path(&uri);
        self.sync_symbols(&uri);
        let encoding = self.encoding;
        let document = self.documents.get_mut(&uri)?;
        let root = document.parse().syntax();
        let offset = u32::from(document.mapper(encoding).offset(position.position));
        let project = match &path {
            Some(path) => self.workspace.project_for(path),
            None => &self.workspace.loose,
        };
        let found = signature_help(&project.index, &root, offset)?;
        let markdown = |text: String| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: text,
            })
        };
        let signatures: Vec<SignatureInformation> = found
            .signatures
            .into_iter()
            .map(|signature| SignatureInformation {
                label: signature.label,
                documentation: signature.documentation.map(markdown),
                parameters: Some(
                    signature
                        .parameters
                        .into_iter()
                        .map(|parameter| ParameterInformation {
                            label: ParameterLabel::Simple(parameter.label),
                            documentation: parameter.documentation.map(markdown),
                        })
                        .collect(),
                ),
                active_parameter: signature.active_parameter.map(|active| active as u32),
            })
            .collect();
        let active_parameter = signatures
            .get(found.active_signature)
            .and_then(|signature| signature.active_parameter);
        Some(SignatureHelp {
            signatures,
            active_signature: Some(found.active_signature as u32),
            active_parameter,
        })
    }
}
