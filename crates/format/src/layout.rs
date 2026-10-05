//! Lays a token stream out: decides every gap between two significant tokens and writes it as text.
//! Only the gaps are ever produced, so the tokens themselves cannot change.

use php_syntax::SyntaxKind;
use php_syntax::SyntaxKind::*;

use crate::model::Model;
use crate::options::FormatOptions;
use crate::rules::{Braces, Rule, braces, rule};
use crate::wrap::Forced;

/// An item that began on a line of its own and may continue on the next ones.
struct Item {
    line: usize,
    indent: usize,
}

/// What a bracket opened: the lines inside sit one level below the line it was written on. What began
/// on that same line is already one level in, so its later lines add nothing.
struct Frame {
    open_indent: usize,
    open_line: usize,
    items: Vec<Item>,
    id: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkKind {
    Assign,
    Arrow,
}

/// A token whose column a run of neighboring lines may share.
pub(crate) struct Mark {
    pub position: usize,
    pub line: usize,
    pub column: usize,
    pub kind: MarkKind,
    pub frame: usize,
}

pub(crate) struct Layout {
    /// The text before each significant token. The first is the text before the first token.
    pub gaps: Vec<String>,
    /// The text after the last token.
    pub tail: String,
    pub line: Vec<usize>,
    pub end_line: Vec<usize>,
    pub column: Vec<usize>,
    pub end_column: Vec<usize>,
    pub marks: Vec<Mark>,
    /// The level a new line would have after the probed token.
    pub probe_level: Option<usize>,
}

struct Writer<'a, 'b> {
    m: &'b Model<'a>,
    options: &'b FormatOptions,
    forced: &'b Forced,
    eol: &'static str,
    unit: String,
    frames: Vec<Frame>,
    next_frame: usize,
    line: usize,
    column: usize,
    indent: usize,
}

pub(crate) fn lay_out(m: &Model, options: &FormatOptions, forced: &Forced, probe: Option<usize>) -> Layout {
    let eol = if m.text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut writer = Writer {
        m,
        options,
        forced,
        eol,
        unit: options.indent.unit(),
        frames: vec![Frame {
            open_indent: 0,
            open_line: 0,
            items: Vec::new(),
            id: 0,
        }],
        next_frame: 1,
        line: 0,
        column: 0,
        indent: 0,
    };
    let count = m.sig.len();
    let mut layout = Layout {
        gaps: Vec::with_capacity(count),
        tail: String::new(),
        line: Vec::with_capacity(count),
        end_line: Vec::with_capacity(count),
        column: Vec::with_capacity(count),
        end_column: Vec::with_capacity(count),
        marks: Vec::new(),
        probe_level: None,
    };
    for position in 0..count {
        let gap = if position == 0 {
            m.text[..m.leaf(0).start].to_string()
        } else {
            writer.gap(position - 1, Some(position))
        };
        writer.advance(&gap);
        layout.gaps.push(gap);
        let leaf = m.leaf(position);
        layout.line.push(writer.line);
        layout.column.push(writer.column);
        writer.push_items(position);
        writer.advance(&m.text[leaf.start..leaf.end]);
        layout.end_line.push(writer.line);
        layout.end_column.push(writer.column);
        writer.mark(&mut layout, position);
        writer.bracket(position);
        writer.pop_items(position);
        if probe == Some(position) {
            layout.probe_level = Some(writer.level_after());
        }
    }
    layout.tail = match count {
        0 => m.text.to_string(),
        _ => writer.gap(count - 1, None),
    };
    layout
}

fn is_closer(kind: SyntaxKind) -> bool {
    matches!(kind, RPAREN | RBRACKET | RBRACE)
}

fn is_opener(kind: SyntaxKind) -> bool {
    matches!(kind, LPAREN | LBRACKET | LBRACE | HASH_BRACKET)
}

impl Writer<'_, '_> {
    fn advance(&mut self, text: &str) {
        let tab = self.options.indent.width();
        for character in text.chars() {
            match character {
                '\n' => {
                    self.line += 1;
                    self.column = 0;
                }
                '\t' => self.column += tab,
                _ => self.column += 1,
            }
        }
    }

    fn frame(&mut self) -> &mut Frame {
        let last = self.frames.len() - 1;
        &mut self.frames[last]
    }

    fn push_items(&mut self, position: usize) {
        let (line, indent) = (self.line, self.indent);
        let m = self.m;
        let frame = self.frame();
        for _ in &m.starts[position] {
            frame.items.push(Item { line, indent });
        }
    }

    fn pop_items(&mut self, position: usize) {
        let ends = self.m.ends[position];
        let frame = self.frame();
        for _ in 0..ends {
            frame.items.pop();
        }
    }

    fn bracket(&mut self, position: usize) {
        if self.m.leaf(position).opaque.is_some() {
            return;
        }
        let kind = self.m.kind(position);
        if is_opener(kind) {
            let id = self.next_frame;
            self.next_frame += 1;
            let open_indent = match self.aligns_with_owner(position) {
                true => self.frames[self.frames.len() - 1]
                    .items
                    .last()
                    .map_or(self.indent, |item| item.indent),
                false => self.indent,
            };
            self.frames.push(Frame {
                open_indent,
                open_line: self.line,
                items: Vec::new(),
                id,
            });
        } else if is_closer(kind) && self.frames.len() > 1 {
            self.frames.pop();
        }
    }

    fn mark(&self, layout: &mut Layout, position: usize) {
        let m = self.m;
        let kind = match (m.kind(position), m.parent(position)) {
            (ASSIGN, ASSIGN_EXPR) if self.options.align_assignments && m.grandparent(position) == EXPR_STATEMENT => {
                MarkKind::Assign
            }
            (FAT_ARROW, ARRAY_ITEM) if self.options.align_array_arrows => MarkKind::Arrow,
            _ => return,
        };
        let node_start = usize::from(m.leaf(position).parent.text_range().start());
        let first = m.sig.partition_point(|&leaf| m.leaves[leaf].start < node_start);
        if layout.line[first.min(position)] != layout.line[position] || layout.gaps[position].contains('\n') {
            return;
        }
        layout.marks.push(Mark {
            position,
            line: layout.line[position],
            column: layout.column[position],
            kind,
            frame: self.frames.last().map_or(0, |frame| frame.id),
        });
    }

    /// The level of a line that begins with the token at `position`.
    fn level_of(&self, position: usize) -> usize {
        let frame = &self.frames[self.frames.len() - 1];
        let kind = self.m.kind(position);
        if is_closer(kind) && self.frames.len() > 1 && self.m.leaf(position).opaque.is_none() {
            return frame.open_indent;
        }
        if self.aligns_with_owner(position) {
            if let Some(item) = frame.items.last() {
                return item.indent;
            }
        }
        self.inner_level(self.line + 1)
    }

    /// The level of a line inside the open bracket: one below the line it was opened on, and one more
    /// for every earlier line an unfinished statement or member began on.
    fn inner_level(&self, line: usize) -> usize {
        let frame = &self.frames[self.frames.len() - 1];
        let mut distinct = 0;
        let mut last = None;
        let nested = self.frames.len() > 1;
        for item in frame
            .items
            .iter()
            .filter(|item| item.line < line && !(nested && item.line == frame.open_line))
        {
            if last != Some(item.line) {
                distinct += 1;
                last = Some(item.line);
            }
        }
        let base = if self.frames.len() > 1 {
            frame.open_indent + 1
        } else {
            0
        };
        base + distinct
    }

    fn level_after(&self) -> usize {
        self.inner_level(self.line + 1)
    }

    /// Tokens that sit under the start of the statement or declaration they belong to.
    fn aligns_with_owner(&self, position: usize) -> bool {
        let m = self.m;
        match m.kind(position) {
            ELSE_KW | ELSEIF_KW | CATCH_KW | FINALLY_KW | ENDIF_KW | ENDWHILE_KW | ENDFOR_KW | ENDFOREACH_KW
            | ENDSWITCH_KW | ENDDECLARE_KW => true,
            WHILE_KW => m.parent(position) == DO_WHILE_STATEMENT,
            LBRACE => matches!(
                braces(m, position),
                Braces::Class | Braces::Function | Braces::Control | Braces::Switch
            ),
            _ => false,
        }
    }

    fn newline(&self, blank: usize, level: usize) -> String {
        let mut out = String::new();
        for _ in 0..=blank {
            out.push_str(self.eol);
        }
        out.push_str(&self.unit.repeat(level));
        out
    }

    /// Writes the trivia between the token at `p` and the one at `n`, or the end of the file.
    fn gap(&mut self, p: usize, n: Option<usize>) -> String {
        let m = self.m;
        let from = m.sig[p] + 1;
        let to = n.map_or(m.leaves.len(), |n| m.sig[n]);
        let trivia = &m.leaves[from..to];
        let inside_string = n.is_some_and(
            |n| matches!((m.leaf(p).opaque, m.leaf(n).opaque), (Some(left), Some(right)) if left == right),
        );
        if inside_string {
            return m.text[trivia.first().map_or(0, |leaf| leaf.start)..trivia.last().map_or(0, |leaf| leaf.end)]
                .to_string();
        }
        let mut segments: Vec<String> = vec![String::new()];
        let mut comments: Vec<&str> = Vec::new();
        for leaf in trivia {
            let text = &m.text[leaf.start..leaf.end];
            if leaf.kind == WHITESPACE {
                if let Some(last) = segments.last_mut() {
                    last.push_str(text);
                }
            } else {
                comments.push(text);
                segments.push(String::new());
            }
        }
        if n.is_none() && matches!(m.kind(p), CLOSE_TAG | INLINE_HTML | HALT_DATA | OPEN_TAG) {
            return m.text[m.leaf(p).end..].to_string();
        }
        let mut chosen = match n {
            Some(n) => rule(m, self.options, self.forced, p, n),
            None => Rule::BreakTight,
        };
        if !comments.is_empty() && matches!(chosen, Rule::Join | Rule::JoinTight) {
            chosen = Rule::Keep;
        }
        let open_tag_text = (m.kind(p) == OPEN_TAG).then(|| &m.text[m.leaf(p).start..m.leaf(p).end]);
        if open_tag_text.is_some_and(|text| text.ends_with('\n')) {
            segments[0].insert(0, '\n');
        }
        let newlines: Vec<usize> = segments.iter().map(|segment| segment.matches('\n').count()).collect();
        let wants_break = matches!(chosen, Rule::Break | Rule::BreakTight | Rule::BreakBlank(_));
        let tail = n.is_none();
        let mut breaks: Vec<bool> = newlines.iter().map(|count| *count > 0).collect();
        let last = segments.len() - 1;
        let ends_in_line_comment = comments
            .last()
            .is_some_and(|comment| comment.starts_with("//") || comment.starts_with('#'));
        let before_close_tag = n.is_some_and(|n| m.kind(n) == CLOSE_TAG);
        let line_comment_ends_gap = !comments.is_empty() && ends_in_line_comment && !before_close_tag;
        if line_comment_ends_gap || tail || wants_break && !breaks.iter().any(|flag| *flag) {
            breaks[last] = true;
        }
        let first_break = breaks.iter().position(|flag| *flag);
        let last_break = breaks.iter().rposition(|flag| *flag);

        if comments.is_empty() && !breaks[0] {
            return match chosen {
                Rule::Space | Rule::Join => " ".to_string(),
                Rule::NoSpace | Rule::JoinTight => String::new(),
                _ => segments.swap_remove(0),
            };
        }

        let level_n = n.map(|n| self.level_of(n)).unwrap_or(0);
        let level_comment = match n {
            Some(n) if is_closer(m.kind(n)) && self.frames.len() > 1 && m.leaf(n).opaque.is_none() => {
                self.inner_level(self.line + 1)
            }
            Some(_) => level_n,
            None => 0,
        };
        let mut out = String::new();
        for (index, segment) in segments.iter().enumerate() {
            let level = if index == last { level_n } else { level_comment };
            if breaks[index] {
                let original = newlines[index];
                let kept = original.saturating_sub(1).min(1);
                let blank = if Some(index) == first_break {
                    match chosen {
                        Rule::BreakBlank(count) => count,
                        Rule::BreakTight => 0,
                        _ => kept,
                    }
                } else if Some(index) == last_break && chosen == Rule::BreakTight {
                    0
                } else {
                    kept
                };
                if tail && index == last {
                    out.push_str(self.eol);
                } else {
                    let text = self.newline(blank, level);
                    let consumed = match open_tag_text {
                        Some(tag) if index == 0 && tag.ends_with('\n') => text.strip_prefix(self.eol),
                        Some(tag) if index == 0 && tag.ends_with('\r') => text.strip_prefix('\r'),
                        _ => None,
                    };
                    out.push_str(consumed.unwrap_or(&text));
                }
            } else {
                out.push_str(segment);
            }
            if breaks[index] {
                self.indent = level;
            }
            if let Some(comment) = comments.get(index) {
                let old_indent = segment.rsplit('\n').next().unwrap_or_default();
                if breaks[index] {
                    out.push_str(&reindent(comment, old_indent, &self.unit.repeat(level)));
                } else {
                    out.push_str(comment);
                }
            }
        }
        out
    }
}

/// Moves the later lines of a block comment along with its first.
fn reindent(comment: &str, old: &str, new: &str) -> String {
    if !comment.contains('\n') {
        return comment.to_string();
    }
    let mut out = String::with_capacity(comment.len());
    for (index, line) in comment.split_inclusive('\n').enumerate() {
        let trimmed = line.trim_start_matches([' ', '\t']);
        if index == 0 || line.trim().is_empty() {
            out.push_str(line);
        } else if let Some(rest) = line.strip_prefix(old) {
            out.push_str(new);
            out.push_str(rest);
        } else if trimmed.starts_with('*') {
            out.push_str(new);
            out.push(' ');
            out.push_str(trimmed);
        } else {
            out.push_str(line);
        }
    }
    out
}
