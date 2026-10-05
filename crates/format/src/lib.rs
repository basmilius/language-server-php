//! A PHP formatter that only ever changes whitespace: it decides the text between two tokens and
//! leaves the tokens alone. The defaults are PER Coding Style 2.0.

mod editorconfig;
mod layout;
mod model;
mod options;
mod rules;
mod wrap;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, parse};

pub use crate::editorconfig::{EditorConfig, IndentSize, IndentStyle, LineLength};
use crate::layout::{Layout, lay_out};
use crate::model::Model;
pub use crate::model::Refusal;
pub use crate::options::{BraceStyle, FormatOptions, Indent};
use crate::wrap::Forced;

/// A replacement of a byte range of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

/// The most times a line is broken up again, which is the depth of calls inside calls.
const MAX_WRAP_PASSES: usize = 12;

/// The whole text formatted, or why it is left as it is.
pub fn try_format(text: &str, options: &FormatOptions) -> Result<String, Refusal> {
    let run = Run::new(text, options, false, None)?;
    Ok(apply(text, &run.edits(0, text.len())))
}

/// The layout without the check that the tokens stayed, for finding why a text is refused.
#[doc(hidden)]
pub fn format_unchecked(text: &str, options: &FormatOptions) -> Result<String, Refusal> {
    let mut run = Run::new(text, options, true, None)?;
    run.checked = false;
    Ok(apply(text, &run.edits(0, text.len())))
}

/// The whole text formatted, or `None` for a text that is left as it is.
pub fn format(text: &str, options: &FormatOptions) -> Option<String> {
    try_format(text, options).ok()
}

/// What formatting the whole text changes.
pub fn edits(text: &str, options: &FormatOptions) -> Option<Vec<Edit>> {
    let run = Run::new(text, options, false, None).ok()?;
    Some(run.edits(0, text.len()))
}

/// What formatting changes within a byte range, widened to whole lines. A gap belongs to the line
/// of the token it leads up to. The layout still follows
/// the whole file, so a line comes out as it would in a full format.
pub fn range_edits(text: &str, start: usize, end: usize, options: &FormatOptions) -> Option<Vec<Edit>> {
    if !(text.is_char_boundary(start) && text.is_char_boundary(end)) {
        return None;
    }
    let run = Run::new(text, options, false, None).ok()?;
    Some(run.edits(start, end))
}

/// What typing `typed` at `offset` (just after the character) changes: the line it ended is laid out,
/// and a new line gets the indentation its place asks for. Works on text that is not finished.
pub fn on_type_edits(text: &str, offset: usize, typed: char, options: &FormatOptions) -> Option<Vec<Edit>> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    if typed == '\n' {
        return new_line_edits(text, offset, options);
    }
    let run = Run::new(text, options, true, None).ok()?;
    let start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
    Some(run.edits(start, offset))
}

/// The text with the edits applied, which must not overlap.
pub fn apply(text: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for edit in edits {
        out.push_str(&text[at..edit.start]);
        out.push_str(&edit.text);
        at = edit.end;
    }
    out.push_str(&text[at..]);
    out
}

fn new_line_edits(text: &str, offset: usize, options: &FormatOptions) -> Option<Vec<Edit>> {
    let line_start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |at| offset + at);
    if !text[line_start..line_end].trim().is_empty() {
        return Some(Vec::new());
    }
    let model = Model::build(text, true).ok()?;
    let before = model.sig.partition_point(|&leaf| model.leaves[leaf].end <= line_start);
    let previous = before.checked_sub(1)?;
    let layout = lay_out(&model, options, &Forced::new(), Some(previous));
    let level = layout.probe_level?;
    let indent = options.indent.unit().repeat(level);
    if text[line_start..line_end] == indent {
        return Some(Vec::new());
    }
    Some(vec![Edit {
        start: line_start,
        end: line_end,
        text: indent,
    }])
}

struct Run<'a> {
    model: Model<'a>,
    layout: Layout,
    checked: bool,
}

impl<'a> Run<'a> {
    fn new(
        text: &'a str,
        options: &FormatOptions,
        allow_errors: bool,
        probe: Option<usize>,
    ) -> Result<Run<'a>, Refusal> {
        let model = Model::build(text, allow_errors)?;
        if model.sig.iter().all(|&leaf| model.leaves[leaf].kind == INLINE_HTML) {
            return Err(Refusal::Empty);
        }
        let mut forced = Forced::new();
        let mut layout = lay_out(&model, options, &forced, probe);
        for _ in 0..MAX_WRAP_PASSES {
            if options.line_length == 0 {
                break;
            }
            let wrapped = breaks_to_add(&model, &layout, options, &forced);
            if wrapped.is_empty() {
                break;
            }
            forced.extend(wrapped);
            layout = lay_out(&model, options, &forced, probe);
        }
        align(&mut layout);
        let run = Run {
            model,
            layout,
            checked: true,
        };
        if !allow_errors && !run.keeps_tokens() {
            return Err(Refusal::ChangedTokens);
        }
        Ok(run)
    }

    fn pieces(&self) -> impl Iterator<Item = (usize, usize, &str)> {
        let model = &self.model;
        let layout = &self.layout;
        let count = model.sig.len();
        (1..=count).map(move |position| {
            let start = model.leaf(position - 1).end;
            if position == count {
                (start, model.text.len(), layout.tail.as_str())
            } else {
                (start, model.leaf(position).start, layout.gaps[position].as_str())
            }
        })
    }

    /// The edits of the gaps that touch the lines the byte range is on.
    fn edits(&self, from: usize, to: usize) -> Vec<Edit> {
        let text = self.model.text;
        let from = text[..from.min(text.len())].rfind('\n').map_or(0, |at| at + 1);
        let to = text[to.min(text.len())..].find('\n').map_or(text.len(), |at| to + at);
        self.pieces()
            .filter(|(start, end, new)| &text[*start..*end] != *new && (from..=to).contains(end))
            .map(|(start, end, new)| Edit {
                start,
                end,
                text: new.to_string(),
            })
            .collect()
    }

    /// Whether the formatted text has the very tokens of the original. This is what makes the rules
    /// safe: a gap that joined two tokens into one, or split one, never reaches the client.
    fn keeps_tokens(&self) -> bool {
        let edits = self.edits(0, self.model.text.len());
        let formatted = apply(self.model.text, &edits);
        same_tokens(self.model.text, &formatted)
    }
}

/// The comparison ignores the indentation inside comments, which moves with the code around them.
pub fn same_tokens(left: &str, right: &str) -> bool {
    let (left, right) = (parse(left), parse(right));
    if !right.errors().is_empty() && left.errors().is_empty() {
        return false;
    }
    let tokens = |parsed: &php_syntax::Parse| -> Vec<(SyntaxKind, String)> {
        parsed
            .syntax()
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .filter(|token| token.kind() != WHITESPACE)
            .map(|token| {
                let text = match token.kind() {
                    COMMENT | BLOCK_COMMENT | DOC_COMMENT => {
                        token.text().lines().map(str::trim).collect::<Vec<_>>().join("\n")
                    }
                    _ => token.text().to_string(),
                };
                (token.kind(), text)
            })
            .collect()
    };
    tokens(&left) == tokens(&right)
}

/// The constructs to break over lines so that the long lines get shorter, one for each long line.
fn breaks_to_add(model: &Model, layout: &Layout, options: &FormatOptions, forced: &Forced) -> Vec<(wrap::Wrap, usize)> {
    let limit = options.line_length;
    let count = model.sig.len();
    let mut found = Vec::new();
    let mut start = 0;
    while start < count {
        let line = layout.line[start];
        let mut end = start;
        while end + 1 < count && layout.line[end + 1] == line {
            end += 1;
        }
        let single = (start..=end).all(|position| layout.end_line[position] == line);
        if single && layout.end_column[end] > limit {
            if let Some(choice) = wrap::choose(model, layout, forced, start, end, limit) {
                found.push(choice);
            }
        }
        start = end + 1;
    }
    found
}

/// Pads the marks of neighboring lines to one column.
fn align(layout: &mut Layout) {
    let mut run_start = 0;
    let marks = std::mem::take(&mut layout.marks);
    let continues = |left: &crate::layout::Mark, right: &crate::layout::Mark| {
        left.kind == right.kind && left.frame == right.frame && right.line == left.line + 1
    };
    for index in 0..=marks.len() {
        let ends = index == marks.len() || index > run_start && !continues(&marks[index - 1], &marks[index]);
        if !ends {
            continue;
        }
        let run = &marks[run_start..index];
        if run.len() > 1 {
            let target = run.iter().map(|mark| mark.column).max().unwrap_or(0);
            for mark in run {
                layout.gaps[mark.position].push_str(&" ".repeat(target - mark.column));
            }
        }
        run_start = index;
    }
}

#[cfg(test)]
mod tests;
