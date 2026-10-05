//! Building and applying text edits.

use php_syntax::TextRange;

use crate::completion::TextEdit;

pub fn insert(at: u32, text: impl Into<String>) -> TextEdit {
    TextEdit {
        start: at,
        end: at,
        new_text: text.into(),
    }
}

pub fn replace(range: TextRange, text: impl Into<String>) -> TextEdit {
    TextEdit {
        start: u32::from(range.start()),
        end: u32::from(range.end()),
        new_text: text.into(),
    }
}

pub fn delete(range: TextRange) -> TextEdit {
    replace(range, "")
}

pub fn line_start(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].rfind('\n').map_or(0, |at| at + 1)
}

/// The offset after the line break of the line an offset is on, or the end of the text.
pub fn line_end(text: &str, offset: usize) -> usize {
    let offset = offset.min(text.len());
    text[offset..].find('\n').map_or(text.len(), |at| offset + at + 1)
}

/// The whitespace a line starts with.
pub fn indent_of(text: &str, offset: usize) -> String {
    let start = line_start(text, offset);
    text[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect()
}

/// Removes a range and, when it was alone on its lines, the lines too, along with one of the blank
/// lines around that would be left twice.
pub fn remove_with_lines(text: &str, range: TextRange) -> TextEdit {
    let mut start = usize::from(range.start());
    let mut end = usize::from(range.end());
    let before_is_blank = text[line_start(text, start)..start].trim().is_empty();
    let rest_of_line = &text[end..line_end(text, end).max(end)];
    let after_is_blank = rest_of_line.trim().is_empty();
    if before_is_blank && after_is_blank {
        start = line_start(text, start);
        end = line_end(text, end);
        let blank_before = start >= 2 && text[..start].ends_with("\n\n") || start == 0;
        let blank_after = text[end..].starts_with('\n') || text[end..].starts_with("\r\n");
        if blank_before && blank_after {
            end = line_end(text, end);
        } else if blank_before && start > 0 && text[end..].trim_start().starts_with('}') {
            start -= 1;
        }
    }
    TextEdit {
        start: start as u32,
        end: end as u32,
        new_text: String::new(),
    }
}

/// New `use` lines that land at the same place go in as one edit, in order, since the order of
/// separate inserts at one position is up to the client.
pub fn merge_inserts(mut edits: Vec<TextEdit>) -> Vec<TextEdit> {
    edits.sort_by(|left, right| (left.start, left.new_text.trim()).cmp(&(right.start, right.new_text.trim())));
    let mut merged: Vec<TextEdit> = Vec::new();
    for edit in edits {
        match merged.last_mut() {
            Some(last) if last.start == edit.start && last.end == edit.end && edit.start == edit.end => {
                last.new_text.push_str(edit.new_text.trim_start_matches('\n'));
            }
            _ => merged.push(edit),
        }
    }
    merged
}

/// The text with every edit applied, each against the original text.
pub fn apply(text: &str, edits: &[TextEdit]) -> String {
    let mut ordered: Vec<&TextEdit> = edits.iter().collect();
    ordered.sort_by_key(|edit| std::cmp::Reverse((edit.start, edit.end)));
    let mut out = text.to_string();
    for edit in ordered {
        out.replace_range(edit.start as usize..edit.end as usize, &edit.new_text);
    }
    out
}
