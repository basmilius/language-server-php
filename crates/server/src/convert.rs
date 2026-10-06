//! From what `php-analysis` answers to the shapes of LSP.

use lsp_types::{
    Diagnostic, DiagnosticSeverity, DiagnosticTag, DocumentSymbol, FoldingRange, FoldingRangeKind, Location,
    NumberOrString, Position, PositionEncodingKind, Range, SelectionRange, SymbolInformation, SymbolKind, SymbolTag,
    Uri,
};
use php_analysis::{Fold, FoldKind, LineCol, LineIndex, PositionEncoding, Symbol};
use php_syntax::{TextRange, TextSize};

pub fn encoding_kind(encoding: PositionEncoding) -> PositionEncodingKind {
    match encoding {
        PositionEncoding::Utf8 => PositionEncodingKind::UTF8,
        PositionEncoding::Utf16 => PositionEncodingKind::UTF16,
        PositionEncoding::Utf32 => PositionEncodingKind::UTF32,
    }
}

/// The first of the client's encodings that the server prefers, or UTF-16 which every client has.
pub fn choose_encoding(offered: Option<&[PositionEncodingKind]>) -> PositionEncoding {
    let offered = offered.unwrap_or_default();
    [PositionEncodingKind::UTF8, PositionEncodingKind::UTF32]
        .iter()
        .find(|wanted| offered.contains(wanted))
        .map_or(PositionEncoding::Utf16, |found| {
            if *found == PositionEncodingKind::UTF8 {
                PositionEncoding::Utf8
            } else {
                PositionEncoding::Utf32
            }
        })
}

pub struct Mapper<'a> {
    pub text: &'a str,
    pub index: &'a LineIndex,
    pub encoding: PositionEncoding,
}

impl Mapper<'_> {
    pub fn position(&self, offset: TextSize) -> Position {
        let LineCol { line, col } = self.index.line_col(self.text, offset, self.encoding);
        Position::new(line, col)
    }

    pub fn range(&self, range: TextRange) -> Range {
        Range::new(self.position(range.start()), self.position(range.end()))
    }

    /// A range with something to draw: an empty one, where something is missing, widens to the
    /// character before it, or to the one after it at the start of a line.
    pub fn visible_range(&self, range: TextRange) -> Range {
        if !range.is_empty() {
            return self.range(range);
        }
        let offset = usize::from(range.start());
        if let Some(previous) = self.text[..offset]
            .chars()
            .next_back()
            .filter(|c| !matches!(c, '\n' | '\r'))
        {
            let start = TextSize::from((offset - previous.len_utf8()) as u32);
            return self.range(TextRange::new(start, range.start()));
        }
        if let Some(next) = self.text[offset..].chars().next().filter(|c| !matches!(c, '\n' | '\r')) {
            let end = TextSize::from((offset + next.len_utf8()) as u32);
            return self.range(TextRange::new(range.start(), end));
        }
        self.range(range)
    }

    pub fn offset(&self, position: Position) -> TextSize {
        self.index.offset(
            self.text,
            LineCol {
                line: position.line,
                col: position.character,
            },
            self.encoding,
        )
    }

    pub fn diagnostic(&self, found: &php_analysis::Diagnostic) -> Diagnostic {
        Diagnostic {
            range: self.visible_range(found.range),
            severity: Some(match found.severity {
                php_analysis::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                php_analysis::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                php_analysis::DiagnosticSeverity::Information => DiagnosticSeverity::INFORMATION,
                php_analysis::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
            }),
            code: Some(NumberOrString::String(found.code.to_string())),
            source: Some("php".to_string()),
            message: found.message.clone(),
            tags: diagnostic_tags(found),
            ..Diagnostic::default()
        }
    }
}

fn diagnostic_tags(found: &php_analysis::Diagnostic) -> Option<Vec<DiagnosticTag>> {
    let mut tags = Vec::new();
    if found.deprecated {
        tags.push(DiagnosticTag::DEPRECATED);
    }
    if found.unnecessary {
        tags.push(DiagnosticTag::UNNECESSARY);
    }
    (!tags.is_empty()).then_some(tags)
}

fn symbol_kind(kind: php_analysis::SymbolKind) -> SymbolKind {
    use php_analysis::SymbolKind as Kind;
    match kind {
        Kind::Namespace => SymbolKind::NAMESPACE,
        Kind::Class | Kind::Trait => SymbolKind::CLASS,
        Kind::Interface => SymbolKind::INTERFACE,
        Kind::Enum => SymbolKind::ENUM,
        Kind::Method => SymbolKind::METHOD,
        Kind::Constructor => SymbolKind::CONSTRUCTOR,
        Kind::Property => SymbolKind::PROPERTY,
        Kind::Constant => SymbolKind::CONSTANT,
        Kind::EnumMember => SymbolKind::ENUM_MEMBER,
        Kind::Function => SymbolKind::FUNCTION,
    }
}

/// The detail of a trait says so, since LSP has no kind for it.
fn detail(symbol: &Symbol) -> Option<String> {
    match (symbol.kind, &symbol.detail) {
        (php_analysis::SymbolKind::Trait, None) => Some("trait".to_string()),
        (_, detail) => detail.clone(),
    }
}

#[allow(deprecated)]
pub fn hierarchical_symbols(mapper: &Mapper, symbols: &[Symbol]) -> Vec<DocumentSymbol> {
    symbols
        .iter()
        .map(|symbol| DocumentSymbol {
            name: symbol.name.clone(),
            detail: detail(symbol),
            kind: symbol_kind(symbol.kind),
            tags: symbol.deprecated.then(|| vec![SymbolTag::DEPRECATED]),
            deprecated: None,
            range: mapper.range(symbol.range),
            selection_range: mapper.range(symbol.selection_range),
            children: (!symbol.children.is_empty()).then(|| hierarchical_symbols(mapper, &symbol.children)),
        })
        .collect()
}

#[allow(deprecated)]
pub fn flat_symbols(
    mapper: &Mapper,
    uri: &Uri,
    symbols: &[Symbol],
    container: Option<&str>,
    out: &mut Vec<SymbolInformation>,
) {
    for symbol in symbols {
        out.push(SymbolInformation {
            name: symbol.name.clone(),
            kind: symbol_kind(symbol.kind),
            tags: symbol.deprecated.then(|| vec![SymbolTag::DEPRECATED]),
            deprecated: None,
            location: Location::new(uri.clone(), mapper.range(symbol.range)),
            container_name: container.map(str::to_string),
        });
        flat_symbols(mapper, uri, &symbol.children, Some(&symbol.name), out);
    }
}

/// A fold, with the characters where it starts and ends when a mapper is given: only a client
/// that folds within a line takes those.
pub fn folding_range(mapper: Option<&Mapper>, fold: &Fold) -> FoldingRange {
    let characters = mapper
        .zip(fold.characters)
        .map(|(mapper, (start, end))| (mapper.position(start).character, mapper.position(end).character));
    FoldingRange {
        start_line: fold.start_line,
        start_character: characters.map(|(start, _)| start),
        end_line: fold.end_line,
        end_character: characters.map(|(_, end)| end),
        kind: fold.kind.map(|kind| match kind {
            FoldKind::Comment => FoldingRangeKind::Comment,
            FoldKind::Imports => FoldingRangeKind::Imports,
            FoldKind::Region => FoldingRangeKind::Region,
        }),
        collapsed_text: None,
    }
}

/// A chain from the smallest range to the largest, as LSP nests it: each one's parent is the next.
pub fn selection_chain(mapper: &Mapper, ranges: &[TextRange]) -> SelectionRange {
    let mut parent: Option<Box<SelectionRange>> = None;
    for range in ranges.iter().rev() {
        parent = Some(Box::new(SelectionRange {
            range: mapper.range(*range),
            parent,
        }));
    }
    match parent {
        Some(chain) => *chain,
        None => SelectionRange {
            range: Range::default(),
            parent: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_utf8_when_offered_and_utf16_otherwise() {
        assert_eq!(choose_encoding(None), PositionEncoding::Utf16);
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::UTF16])),
            PositionEncoding::Utf16
        );
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::UTF16, PositionEncodingKind::UTF8])),
            PositionEncoding::Utf8
        );
        assert_eq!(
            choose_encoding(Some(&[PositionEncodingKind::UTF32])),
            PositionEncoding::Utf32
        );
    }

    #[test]
    fn widens_an_empty_range_to_a_character() {
        let text = "$a = ;\nx";
        let index = LineIndex::new(text);
        let mapper = Mapper {
            text,
            index: &index,
            encoding: PositionEncoding::Utf16,
        };
        let widened = mapper.visible_range(TextRange::empty(TextSize::from(5)));
        assert_eq!((widened.start.character, widened.end.character), (4, 5));
        let at_line_start = mapper.visible_range(TextRange::empty(TextSize::from(7)));
        assert_eq!(
            (
                at_line_start.start.line,
                at_line_start.start.character,
                at_line_start.end.character
            ),
            (1, 0, 1)
        );
    }
}
