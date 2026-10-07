//! The pieces of a PHP string expression as SQL sees them: the text of its literals with their
//! escapes, and a hole for every interpolation, concatenated expression and format placeholder.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode, TextRange};
use sql_embed::{EscapeStyle, Fragment, FragmentKind, HoleKind, ScopeTable, Span};

use super::sinks::Format;
use crate::ast::{end, start, text_of};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Item {
    Text {
        raw: String,
        start: u32,
        style: EscapeStyle,
        /// The indentation of a heredoc's closing marker, which PHP leaves out of every line, and
        /// whether the text starts a line.
        indent: Option<(u32, bool)>,
    },
    /// A `%%` of a format, which is one `%`.
    Percent {
        span: Span,
        style: EscapeStyle,
    },
    Hole {
        span: Span,
        kind: Option<HoleKind>,
    },
}

/// A string expression read as pieces, before what it is in SQL is decided.
#[derive(Clone, Debug)]
pub(crate) struct Pieces {
    items: Vec<Item>,
    /// The labels of the heredocs and nowdocs in it.
    pub labels: Vec<String>,
    pub range: TextRange,
}

/// Whether a node is a string literal, an interpolated string or a heredoc.
pub(crate) fn is_string_leaf(node: &SyntaxNode) -> bool {
    match node.kind() {
        LITERAL => node
            .children_with_tokens()
            .any(|element| element.kind() == STRING_LITERAL),
        INTERPOLATED_STRING | HEREDOC => true,
        _ => false,
    }
}

pub(crate) fn is_concat(node: &SyntaxNode) -> bool {
    node.kind() == BINARY_EXPR
        && node
            .children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .any(|token| token.kind() == DOT)
}

/// Whether an expression is made of strings: a literal, or a concatenation or parentheses with one
/// in them.
pub(crate) fn is_string_expression(node: &SyntaxNode) -> bool {
    if is_string_leaf(node) {
        return true;
    }
    match node.kind() {
        PAREN_EXPR => node.children().next().is_some_and(|inner| is_string_expression(&inner)),
        _ if is_concat(node) => node.children().any(|operand| is_string_expression(&operand)),
        _ => false,
    }
}

/// The outermost string expression a string literal is part of.
pub(crate) fn root_of(leaf: &SyntaxNode) -> SyntaxNode {
    let mut node = leaf.clone();
    while let Some(parent) = node.parent() {
        if parent.kind() == PAREN_EXPR || is_concat(&parent) {
            node = parent;
        } else {
            break;
        }
    }
    node
}

/// What a hole holds when the expression says it: a number, or a list from `implode()`.
fn hinted(expression: &SyntaxNode) -> Option<HoleKind> {
    match expression.kind() {
        LITERAL => Some(HoleKind::Value),
        CAST_EXPR => {
            let cast = text_of(expression).to_ascii_lowercase();
            ["(int", "(integer", "(float", "(double", "(bool"]
                .iter()
                .any(|prefix| cast.starts_with(prefix))
                .then_some(HoleKind::Value)
        }
        CALL_EXPR => {
            let callee = expression.children().next()?;
            let name = callee
                .descendants()
                .filter(|node| node.kind() == NAME)
                .last()
                .map(|name| text_of(&name).trim_start_matches('\\').to_ascii_lowercase())?;
            match name.as_str() {
                "implode" | "join" => Some(HoleKind::List),
                "intval" | "floatval" | "count" | "time" | "quote" | "number_format" | "abs" | "round" | "max"
                | "min" => Some(HoleKind::Value),
                _ => None,
            }
        }
        _ => None,
    }
}

fn hole(out: &mut Vec<Item>, node: &SyntaxNode) {
    out.push(Item::Hole {
        span: Span::new(start(node), end(node)),
        kind: hinted(node),
    });
}

/// The text of a quoted literal without its quotes and its `b` prefix, where it starts, and how it
/// escapes.
fn quoted(node: &SyntaxNode) -> Option<(String, u32, EscapeStyle)> {
    let token = node
        .children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|token| token.kind() == STRING_LITERAL)?;
    let text = token.text();
    let prefix = usize::from(text.starts_with(['b', 'B']));
    let quote = text.as_bytes().get(prefix).copied()?;
    let style = match quote {
        b'\'' => EscapeStyle::SingleQuoted,
        b'"' => EscapeStyle::DoubleQuoted,
        _ => return None,
    };
    let body_start = prefix + 1;
    let body_end = if text.len() > body_start && text.ends_with(quote as char) {
        text.len() - 1
    } else {
        text.len()
    };
    let start = u32::from(token.text_range().start()) + body_start as u32;
    Some((text[body_start..body_end].to_string(), start, style))
}

/// The label of a heredoc or nowdoc, and whether it is a nowdoc.
fn heredoc_label(node: &SyntaxNode) -> Option<(String, bool)> {
    let start = node
        .children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|token| token.kind() == HEREDOC_START)?;
    let text = start
        .text()
        .trim_start_matches(['b', 'B'])
        .trim_start_matches("<<<")
        .trim();
    let nowdoc = text.starts_with('\'');
    Some((text.trim_matches(['\'', '"']).to_string(), nowdoc))
}

fn collect_heredoc(node: &SyntaxNode, out: &mut Vec<Item>, labels: &mut Vec<String>) {
    let Some((label, nowdoc)) = heredoc_label(node) else {
        return hole(out, node);
    };
    labels.push(label);
    let style = if nowdoc {
        EscapeStyle::Verbatim
    } else {
        EscapeStyle::Heredoc
    };
    let elements: Vec<SyntaxElement> = node.children_with_tokens().collect();
    let indent = elements
        .iter()
        .find(|element| element.kind() == HEREDOC_END)
        .and_then(|element| element.as_token().cloned())
        .map_or(0, |token| {
            token
                .text()
                .bytes()
                .take_while(|byte| matches!(byte, b' ' | b'\t'))
                .count() as u32
        });
    let body_start = elements
        .iter()
        .find(|element| element.kind() == HEREDOC_START)
        .map_or(start(node), |element| u32::from(element.text_range().end()));
    let body: Vec<&SyntaxElement> = elements
        .iter()
        .filter(|element| !matches!(element.kind(), HEREDOC_START | HEREDOC_END))
        .collect();
    let first = out.len();
    let mut line_start = true;
    for (position, element) in body.iter().enumerate() {
        match element {
            SyntaxElement::Token(token) if token.kind() == STRING_CONTENT => {
                let mut raw = token.text().to_string();
                // The line break before the closing marker is not part of the value.
                if position + 1 == body.len() {
                    if let Some(stripped) = raw.strip_suffix('\n') {
                        raw = stripped.strip_suffix('\r').unwrap_or(stripped).to_string();
                    }
                }
                out.push(Item::Text {
                    raw,
                    start: u32::from(token.text_range().start()),
                    style,
                    indent: Some((indent, line_start)),
                });
                line_start = token.text().ends_with('\n');
            }
            SyntaxElement::Node(child) => {
                hole(out, child);
                line_start = false;
            }
            SyntaxElement::Token(_) => {}
        }
    }
    if out.len() == first {
        out.push(Item::Text {
            raw: String::new(),
            start: body_start,
            style,
            indent: Some((indent, true)),
        });
    }
}

fn collect(node: &SyntaxNode, out: &mut Vec<Item>, labels: &mut Vec<String>) {
    match node.kind() {
        LITERAL => match quoted(node) {
            Some((raw, start, style)) => out.push(Item::Text {
                raw,
                start,
                style,
                indent: None,
            }),
            None => hole(out, node),
        },
        INTERPOLATED_STRING => {
            let mut any = false;
            for element in node.children_with_tokens() {
                match element {
                    SyntaxElement::Token(token) if token.kind() == STRING_CONTENT => {
                        any = true;
                        out.push(Item::Text {
                            raw: token.text().to_string(),
                            start: u32::from(token.text_range().start()),
                            style: EscapeStyle::DoubleQuoted,
                            indent: None,
                        });
                    }
                    SyntaxElement::Node(child) => {
                        any = true;
                        hole(out, &child);
                    }
                    SyntaxElement::Token(_) => {}
                }
            }
            if !any {
                out.push(Item::Text {
                    raw: String::new(),
                    start: start(node) + 1,
                    style: EscapeStyle::DoubleQuoted,
                    indent: None,
                });
            }
        }
        HEREDOC => collect_heredoc(node, out, labels),
        PAREN_EXPR if is_string_expression(node) => {
            for child in node.children() {
                collect(&child, out, labels);
            }
        }
        _ if is_concat(node) => {
            for child in node.children() {
                collect(&child, out, labels);
            }
        }
        _ => hole(out, node),
    }
}

/// A conversion of `printf()` or of `wpdb::prepare()` at the start of `raw`: its length and the
/// hole it is, or `None` for `%%`.
fn placeholder(raw: &[u8], format: Format) -> Option<(usize, Option<Option<HoleKind>>)> {
    if raw.first() != Some(&b'%') {
        return None;
    }
    if raw.get(1) == Some(&b'%') {
        return Some((2, None));
    }
    let mut at = 1;
    let digits = |at: &mut usize| {
        while raw.get(*at).is_some_and(u8::is_ascii_digit) {
            *at += 1;
        }
    };
    let mark = at;
    digits(&mut at);
    if raw.get(at) == Some(&b'$') && at > mark {
        at += 1;
    } else {
        at = mark;
    }
    loop {
        match raw.get(at) {
            Some(b'-' | b'+' | b' ' | b'0') => at += 1,
            Some(b'\'') if raw.len() > at + 1 => at += 2,
            _ => break,
        }
    }
    digits(&mut at);
    if raw.get(at) == Some(&b'.') {
        at += 1;
        digits(&mut at);
    }
    let kind = match (raw.get(at)?, format) {
        (b's', _) => None,
        (b'i', Format::Wpdb) => Some(HoleKind::Identifier),
        (b'd' | b'u' | b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'b' | b'o' | b'x' | b'X' | b'c', Format::Printf)
        | (b'd' | b'f' | b'F', Format::Wpdb) => Some(HoleKind::Value),
        _ => return None,
    };
    // `wpdb::prepare()` quotes what `%s` gives it.
    let kind = match (kind, format) {
        (None, Format::Wpdb) => Some(HoleKind::Value),
        (kind, _) => kind,
    };
    Some((at + 1, Some(kind)))
}

/// The text items split at the placeholders of a format.
fn split_formats(items: Vec<Item>, format: Format) -> Vec<Item> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let Item::Text {
            raw,
            start,
            style,
            indent,
        } = &item
        else {
            out.push(item);
            continue;
        };
        let bytes = raw.as_bytes();
        let mut run = 0;
        let mut at = 0;
        // A piece after a placeholder never starts a line of a heredoc.
        let after_placeholder = |run: usize| indent.map(|(indent, line_start)| (indent, line_start && run == 0));
        while at < bytes.len() {
            match placeholder(&bytes[at..], format) {
                Some((length, found)) => {
                    if run < at {
                        out.push(Item::Text {
                            raw: raw[run..at].to_string(),
                            start: start + run as u32,
                            style: *style,
                            indent: after_placeholder(run),
                        });
                    }
                    let span = Span::new(start + at as u32, start + (at + length) as u32);
                    out.push(match found {
                        None => Item::Percent { span, style: *style },
                        Some(kind) => Item::Hole { span, kind },
                    });
                    at += length;
                    run = at;
                }
                None => at += 1,
            }
        }
        if run < bytes.len() || raw.is_empty() {
            out.push(Item::Text {
                raw: raw[run..].to_string(),
                start: start + run as u32,
                style: *style,
                indent: after_placeholder(run),
            });
        }
    }
    out
}

/// The pieces of a string expression, with the placeholders of a format as holes.
pub(crate) fn pieces(expression: &SyntaxNode, format: Option<Format>) -> Pieces {
    let mut items = Vec::new();
    let mut labels = Vec::new();
    collect(expression, &mut items, &mut labels);
    if let Some(format) = format {
        items = split_formats(items, format);
    }
    Pieces {
        items,
        labels,
        range: expression.text_range(),
    }
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'`' | b'$')
}

/// What a hole is from the text around it: inside a string of SQL a value, inside a quoted name
/// or glued to the letters of one a name, after an operator a value, after `IN (` a list.
fn by_context(before: &str, after: &str, previous_is_hole: bool, next_is_hole: bool) -> HoleKind {
    let (mut single, mut double, mut backtick) = (false, false, false);
    for byte in before.bytes() {
        match byte {
            b'\'' if !double && !backtick => single = !single,
            b'"' if !single && !backtick => double = !double,
            b'`' if !single && !double => backtick = !backtick,
            _ => {}
        }
    }
    if single || double {
        return HoleKind::Value;
    }
    if backtick {
        return HoleKind::Identifier;
    }
    let glued_before = !previous_is_hole && before.bytes().last().is_some_and(is_word);
    let glued_after = !next_is_hole && after.bytes().next().is_some_and(is_word);
    if glued_before || glued_after {
        return HoleKind::Identifier;
    }
    if previous_is_hole {
        return HoleKind::Unknown;
    }
    let trimmed = before.trim_end();
    if trimmed.ends_with(['=', '<', '>', '+', '-', '*', '/', '%']) {
        return HoleKind::Value;
    }
    let upper = trimmed.to_ascii_uppercase();
    if upper.ends_with("IN (") || upper.ends_with("IN(") {
        return HoleKind::List;
    }
    let last_word = upper
        .rsplit(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .next()
        .unwrap_or_default();
    if matches!(last_word, "LIMIT" | "OFFSET" | "LIKE" | "ILIKE" | "BETWEEN") {
        return HoleKind::Value;
    }
    HoleKind::Unknown
}

impl Pieces {
    pub fn has_holes(&self) -> bool {
        self.items.iter().any(|item| matches!(item, Item::Hole { .. }))
    }

    pub fn has_text(&self) -> bool {
        self.items.iter().any(|item| matches!(item, Item::Text { .. }))
    }

    /// The text of the literals, holes left out, for a first look at what the string holds.
    pub fn plain_text(&self) -> String {
        self.items
            .iter()
            .map(|item| match item {
                Item::Text { raw, .. } => raw.as_str(),
                Item::Percent { .. } => "%",
                Item::Hole { .. } => " ",
            })
            .collect()
    }

    /// The end of the last piece in the host.
    fn end(&self) -> u32 {
        match self.items.last() {
            Some(Item::Text { raw, start, .. }) => start + raw.len() as u32,
            Some(Item::Percent { span, .. } | Item::Hole { span, .. }) => span.end,
            None => u32::from(self.range.end()),
        }
    }

    /// The fragment of a kind, with the tables in scope and, for a string that more is appended to
    /// afterwards, a hole at its end that stands for what comes.
    pub fn fragment(&self, kind: FragmentKind, tables: &[ScopeTable], appended: bool) -> Fragment {
        let mut fragment = Fragment::new(kind);
        let texts: Vec<Option<&str>> = self
            .items
            .iter()
            .map(|item| match item {
                Item::Text { raw, .. } => Some(raw.as_str()),
                Item::Percent { .. } => Some("%"),
                Item::Hole { .. } => None,
            })
            .collect();
        for (position, item) in self.items.iter().enumerate() {
            match item {
                Item::Text {
                    raw,
                    start,
                    style,
                    indent,
                } => match indent {
                    Some((indent, line_start)) => {
                        fragment.literal_dedented(raw, *start, *style, *indent, *line_start);
                    }
                    None => {
                        fragment.literal(raw, *start, *style);
                    }
                },
                Item::Percent { span, style } => {
                    fragment.escape("%", *span, *style);
                }
                Item::Hole { span, kind } => {
                    let kind = match kind {
                        Some(HoleKind::List | HoleKind::Unknown) | None => {
                            let before: String = texts[..position].iter().map(|text| text.unwrap_or(" ")).collect();
                            let after = texts.get(position + 1).copied().flatten().unwrap_or_default();
                            let previous_is_hole = position > 0 && texts[position - 1].is_none();
                            let next_is_hole = texts.get(position + 1).is_some_and(Option::is_none);
                            let found = by_context(&before, after, previous_is_hole, next_is_hole);
                            // A list from `implode()` is one only inside parentheses; after
                            // `VALUES` it is rows, which only an open hole reads.
                            let in_parentheses = before.trim_end().ends_with('(');
                            match (kind, found) {
                                (Some(HoleKind::List), HoleKind::Unknown | HoleKind::Value) if in_parentheses => {
                                    HoleKind::List
                                }
                                (Some(HoleKind::List), HoleKind::Value) => HoleKind::Unknown,
                                (_, found) => found,
                            }
                        }
                        Some(kind) => *kind,
                    };
                    fragment.hole(*span, kind);
                }
            }
        }
        if appended {
            let end = self.end();
            fragment.hole(Span::empty(end), HoleKind::Unknown);
        }
        for table in tables {
            fragment.table(table.clone());
        }
        fragment
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use php_syntax::parse;

    fn first_string(code: &str) -> SyntaxNode {
        let root = parse(code).syntax();
        let leaf = root.descendants().find(is_string_leaf).expect("a string");
        root_of(&leaf)
    }

    fn sql(code: &str, format: Option<Format>) -> String {
        let pieces = pieces(&first_string(code), format);
        let fragment = pieces.fragment(FragmentKind::Statements, &[], false);
        let env = sql_embed::Environment::new(sql_embed::Settings::default(), None, None);
        sql_embed::Analysis::new(&env, &fragment).sql().to_string()
    }

    #[test]
    fn reads_literals_concatenations_and_interpolations() {
        assert_eq!(
            sql(r#"<?php $x = 'SELECT * FROM t WHERE a = \'x\' AND id = ' . $id;"#, None),
            "SELECT * FROM t WHERE a = 'x' AND id = ?"
        );
        assert_eq!(
            sql(
                r#"<?php $x = "SELECT * FROM {$prefix}users WHERE id = {$user->id} AND n = \"$name\"";"#,
                None
            ),
            r#"SELECT * FROM hole__1users WHERE id = ? AND n = "?""#
        );
        assert_eq!(
            sql(
                "<?php $x = 'SELECT * FROM t WHERE id IN (' . implode(',', $ids) . ')';",
                None
            ),
            "SELECT * FROM t WHERE id IN (hole__1)"
        );
        assert_eq!(sql("<?php $x = b'SELECT 1';", None), "SELECT 1");
    }

    #[test]
    fn leaves_the_indentation_of_a_heredoc_out() {
        let code = "<?php\n$q = <<<SQL\n    SELECT a\n      FROM t WHERE b = {$c->d}\n    SQL;\n";
        assert_eq!(sql(code, None), "SELECT a\n  FROM t WHERE b = ?");
        let code = "<?php\n$q = <<<'SQL'\n  SELECT 1\n  SQL;\n";
        assert_eq!(sql(code, None), "SELECT 1");
        let code = "<?php\n$q = <<<SQL\nSQL;\n";
        assert_eq!(sql(code, None), "");
    }

    #[test]
    fn reads_the_placeholders_of_a_format() {
        assert_eq!(
            sql(
                "<?php sprintf('SELECT * FROM %s WHERE id = %d AND p LIKE \\'%%x\\'', $t, $id);",
                Some(Format::Printf)
            ),
            "SELECT * FROM hole__1 WHERE id = ? AND p LIKE '%x'"
        );
        assert_eq!(
            sql(
                "<?php $wpdb->prepare('SELECT * FROM %i WHERE a = %s AND b = %1$d', $t, $a);",
                Some(Format::Wpdb)
            ),
            "SELECT * FROM hole__1 WHERE a = ? AND b = ?"
        );
    }

    #[test]
    fn tells_a_value_from_a_name_by_the_text_around_it() {
        assert_eq!(
            by_context("SELECT * FROM t WHERE id = ", "", false, false),
            HoleKind::Value
        );
        assert_eq!(
            by_context("SELECT * FROM ", "users", false, false),
            HoleKind::Identifier
        );
        assert_eq!(
            by_context("SELECT * FROM t_", " WHERE", false, false),
            HoleKind::Identifier
        );
        assert_eq!(
            by_context("SELECT * FROM t WHERE a = '", "'", false, false),
            HoleKind::Value
        );
        assert_eq!(by_context("SELECT * FROM `", "`", false, false), HoleKind::Identifier);
        assert_eq!(
            by_context("SELECT * FROM t WHERE a IN (", ")", false, false),
            HoleKind::List
        );
        assert_eq!(by_context("SELECT * FROM t LIMIT ", "", false, false), HoleKind::Value);
        assert_eq!(
            by_context("SELECT * FROM t ", " ORDER BY a", false, false),
            HoleKind::Unknown
        );
    }
}
