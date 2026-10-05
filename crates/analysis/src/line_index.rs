use php_syntax::{TextRange, TextSize};

/// How the columns of a position count characters, as LSP lets client and server agree.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PositionEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

/// A zero-based line and a column in the units of a [`PositionEncoding`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineCol {
    pub line: u32,
    pub col: u32,
}

/// Maps byte offsets of a text to lines and columns and back.
#[derive(Clone, Debug)]
pub struct LineIndex {
    line_starts: Vec<u32>,
    len: u32,
}

impl LineIndex {
    pub fn new(text: &str) -> LineIndex {
        let mut line_starts = vec![0];
        let bytes = text.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            match bytes[index] {
                b'\n' => line_starts.push(index as u32 + 1),
                b'\r' if bytes.get(index + 1) != Some(&b'\n') => line_starts.push(index as u32 + 1),
                _ => {}
            }
            index += 1;
        }
        LineIndex {
            line_starts,
            len: text.len() as u32,
        }
    }

    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }

    /// The byte offset where a line starts, or the end of the text for a line past it.
    pub fn line_start(&self, line: u32) -> u32 {
        self.line_starts.get(line as usize).copied().unwrap_or(self.len)
    }

    pub fn line_of(&self, offset: u32) -> u32 {
        match self.line_starts.binary_search(&offset) {
            Ok(line) => line as u32,
            Err(next) => next as u32 - 1,
        }
    }

    pub fn line_col(&self, text: &str, offset: TextSize, encoding: PositionEncoding) -> LineCol {
        let offset = u32::from(offset).min(self.len);
        let line = self.line_of(offset);
        let start = self.line_start(line);
        let col = match encoding {
            PositionEncoding::Utf8 => offset - start,
            PositionEncoding::Utf16 => text[start as usize..offset as usize]
                .chars()
                .map(|c| c.len_utf16() as u32)
                .sum(),
            PositionEncoding::Utf32 => text[start as usize..offset as usize].chars().count() as u32,
        };
        LineCol { line, col }
    }

    /// The offset of a position. A column past the end of its line lands on the line's end, and a
    /// line past the text on the end of the text, which is how LSP wants a bad position read.
    pub fn offset(&self, text: &str, position: LineCol, encoding: PositionEncoding) -> TextSize {
        if position.line >= self.line_count() {
            return TextSize::from(self.len);
        }
        let start = self.line_start(position.line) as usize;
        let end = self.line_start(position.line + 1) as usize;
        let line = &text[start..end];
        let line_without_break = line.trim_end_matches(['\n', '\r']);
        let mut remaining = position.col;
        let mut offset = 0usize;
        for character in line_without_break.chars() {
            let width = match encoding {
                PositionEncoding::Utf8 => character.len_utf8() as u32,
                PositionEncoding::Utf16 => character.len_utf16() as u32,
                PositionEncoding::Utf32 => 1,
            };
            if remaining < width {
                break;
            }
            remaining -= width;
            offset += character.len_utf8();
        }
        TextSize::from((start + offset) as u32)
    }

    pub fn range(&self, text: &str, range: TextRange, encoding: PositionEncoding) -> (LineCol, LineCol) {
        (
            self.line_col(text, range.start(), encoding),
            self.line_col(text, range.end(), encoding),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_between_offsets_and_positions_in_every_encoding() {
        let text = "ab\nc\u{e9}\u{1f600}d\r\nlast";
        let index = LineIndex::new(text);
        assert_eq!(index.line_count(), 3);
        let after_emoji = TextSize::from(text.find('d').unwrap() as u32);
        let utf16 = index.line_col(text, after_emoji, PositionEncoding::Utf16);
        assert_eq!((utf16.line, utf16.col), (1, 4));
        let utf8 = index.line_col(text, after_emoji, PositionEncoding::Utf8);
        assert_eq!(utf8.col, 1 + 2 + 4);
        let utf32 = index.line_col(text, after_emoji, PositionEncoding::Utf32);
        assert_eq!(utf32.col, 3);
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16, PositionEncoding::Utf32] {
            let position = index.line_col(text, after_emoji, encoding);
            assert_eq!(index.offset(text, position, encoding), after_emoji);
        }
    }

    #[test]
    fn clamps_positions_past_the_end() {
        let text = "ab\ncd";
        let index = LineIndex::new(text);
        assert_eq!(
            index.offset(text, LineCol { line: 0, col: 99 }, PositionEncoding::Utf16),
            TextSize::from(2)
        );
        assert_eq!(
            index.offset(text, LineCol { line: 9, col: 0 }, PositionEncoding::Utf16),
            TextSize::from(5)
        );
    }

    #[test]
    fn a_lone_carriage_return_breaks_a_line() {
        let index = LineIndex::new("a\rb\r\nc");
        assert_eq!(index.line_count(), 3);
    }
}
