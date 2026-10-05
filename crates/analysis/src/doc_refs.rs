//! The names a doc comment refers to: classes in the types of its tags, the variables and members
//! it names, and the tags themselves. Positions are byte offsets in the file.

use php_index::NameResolver;
use php_index::phpdoc::{TypeContext, parse_type_prefix};

/// How a doc comment uses a name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocItemKind {
    /// `@param`, without the `@`.
    Tag(String),
    /// A class in a type, as written.
    Class(String),
    /// A `$name` after a type, without the `$`. The range includes the `$`.
    Variable(String),
    /// The `$name` of `@property`, which declares a property of the class the comment belongs to.
    PropertyDecl(String),
    /// The name of an `@method` line.
    MethodDecl(String),
    /// `Class::member`, `Class::$member` or `Class::MEMBER` after `@see`.
    Member {
        class: String,
        name: String,
        kind: MemberKind,
        /// The range covers the member name only.
        class_range: (u32, u32),
    },
    /// `function()` after `@see`.
    Function(String),
    /// A name from `@template`, where it is declared and where a type uses it.
    Template(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Method,
    Property,
    Constant,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocItem {
    pub start: u32,
    pub end: u32,
    pub kind: DocItemKind,
}

const KEYWORDS: &[&str] = &[
    "int",
    "integer",
    "float",
    "double",
    "string",
    "bool",
    "boolean",
    "true",
    "false",
    "null",
    "void",
    "never",
    "mixed",
    "object",
    "array",
    "list",
    "iterable",
    "callable",
    "resource",
    "scalar",
    "numeric",
    "self",
    "static",
    "parent",
    "this",
    "of",
    "is",
    "not",
    "min",
    "max",
    "empty",
    "new",
    "extends",
    "as",
    "const",
    "class-string",
    "literal-string",
    "numeric-string",
    "callable-string",
    "non-empty-string",
    "positive-int",
    "negative-int",
    "array-key",
    "key-of",
    "value-of",
    "int-mask",
    "int-mask-of",
    "closed-resource",
    "open-resource",
    "no-return",
    "never-return",
];

/// Everything a doc comment names. `text` is the raw comment and `base` its offset in the file.
pub fn doc_items(text: &str, base: u32) -> Vec<DocItem> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut templates: Vec<String> = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'@' || !is_tag_start(bytes, index) {
            index += 1;
            continue;
        }
        let name_start = index + 1;
        let mut name_end = name_start;
        while name_end < bytes.len()
            && (bytes[name_end].is_ascii_alphanumeric() || matches!(bytes[name_end], b'-' | b'_' | b':' | b'\\'))
        {
            name_end += 1;
        }
        if name_end == name_start {
            index += 1;
            continue;
        }
        let tag = &text[name_start..name_end];
        out.push(DocItem {
            start: base + index as u32,
            end: base + name_end as u32,
            kind: DocItemKind::Tag(tag.to_string()),
        });
        // The tag's content runs to the end of the line, or the closing brace of an inline tag.
        let line_end = text[name_end..]
            .find(['\n', '\r'])
            .map_or(text.len(), |at| name_end + at);
        let inline = index > 0 && bytes[index - 1] == b'{';
        let content_end = if inline {
            text[name_end..line_end].find('}').map_or(line_end, |at| name_end + at)
        } else {
            let stop = text[name_end..line_end].find("*/").map_or(line_end, |at| name_end + at);
            stop.min(line_end)
        };
        let normalized = tag
            .strip_prefix("psalm-")
            .or_else(|| tag.strip_prefix("phpstan-"))
            .unwrap_or(tag);
        let content_end = if inline || !TYPED_TAGS.contains(&normalized) {
            content_end
        } else {
            end_of_open_brackets(text, name_end, content_end)
        };
        let content = &text[name_end..content_end];
        let lead = content.len() - content.trim_start().len();
        let content_start = name_end + lead;
        let content = content.trim();
        let mut cursor = Cursor {
            text,
            base,
            position: content_start,
            end: content_start + content.len(),
            templates: &mut templates,
        };
        cursor.read_tag(normalized, &mut out);
        index = name_end;
    }
    out
}

/// The tags whose content starts with a type, which may run over several lines.
const TYPED_TAGS: &[&str] = &[
    "param",
    "return",
    "var",
    "property",
    "property-read",
    "property-write",
    "throws",
    "extends",
    "implements",
    "use",
    "mixin",
    "template-extends",
    "template-implements",
    "template-use",
    "yield",
    "type",
];

/// Where a type that opens brackets on its line ends: at the line that closes them, as far as no
/// other tag or the end of the comment comes first.
fn end_of_open_brackets(text: &str, from: usize, line_end: usize) -> usize {
    let mut depth: i32 = 0;
    let mut end = line_end;
    let mut line = &text[from..line_end];
    loop {
        for byte in line.bytes() {
            match byte {
                b'<' | b'{' | b'(' | b'[' => depth += 1,
                b'>' | b'}' | b')' | b']' => depth -= 1,
                _ => {}
            }
        }
        if depth <= 0 || end >= text.len() {
            return end;
        }
        let next_start = end + 1;
        let Some(next) = text.get(next_start..) else {
            return end;
        };
        let next_end = next.find('\n').map_or(text.len(), |at| next_start + at);
        line = &text[next_start..next_end];
        let trimmed = line.trim_start_matches([' ', '\t', '*']);
        if trimmed.starts_with('@') || trimmed.starts_with('/') || line.contains("*/") {
            return end;
        }
        end = next_end;
    }
}

/// A tag starts a line (after the comment's own asterisks) or follows an inline `{`.
fn is_tag_start(bytes: &[u8], at: usize) -> bool {
    let mut index = at;
    while index > 0 {
        index -= 1;
        match bytes[index] {
            b' ' | b'\t' | b'*' | b'/' => continue,
            b'\n' | b'\r' => return true,
            b'{' => return true,
            _ => return false,
        }
    }
    true
}

struct Cursor<'a> {
    text: &'a str,
    base: u32,
    position: usize,
    end: usize,
    templates: &'a mut Vec<String>,
}

impl Cursor<'_> {
    fn rest(&self) -> &str {
        &self.text[self.position..self.end]
    }

    fn skip_spaces(&mut self) {
        let rest = self.rest();
        self.position += rest.len() - rest.trim_start().len();
    }

    fn read_tag(&mut self, tag: &str, out: &mut Vec<DocItem>) {
        match tag {
            "param" | "param-out" | "param-immediately-invoked-callable" | "param-later-invoked-callable" => {
                self.read_type(out);
                self.read_variable(out, false);
            }
            "var" | "assert" | "assert-if-true" | "assert-if-false" => {
                self.read_type(out);
                self.read_variable(out, false);
            }
            "return"
            | "throws"
            | "extends"
            | "implements"
            | "use"
            | "mixin"
            | "template-extends"
            | "template-implements"
            | "template-use"
            | "self-out"
            | "this-out"
            | "yield"
            | "type"
            | "import-type" => {
                self.read_type(out);
            }
            "property" | "property-read" | "property-write" => {
                self.read_type(out);
                self.read_variable(out, true);
            }
            "method" => self.read_method(out),
            "template" | "template-covariant" | "template-contravariant" => self.read_template(out),
            "see" | "link" | "uses" | "covers" | "used-by" | "throws-see" => self.read_reference(out),
            _ => {}
        }
    }

    fn read_template(&mut self, out: &mut Vec<DocItem>) {
        let rest = self.rest();
        let word_end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(rest.len());
        if word_end == 0 {
            return;
        }
        let name = rest[..word_end].to_string();
        out.push(DocItem {
            start: self.base + self.position as u32,
            end: self.base + (self.position + word_end) as u32,
            kind: DocItemKind::Template(name.clone()),
        });
        self.templates.push(name);
        self.position += word_end;
        self.skip_spaces();
        let rest = self.rest();
        if let Some(after) = rest
            .strip_prefix("of")
            .filter(|after| after.starts_with(char::is_whitespace))
        {
            self.position += rest.len() - after.len();
            self.skip_spaces();
            self.read_type(out);
        }
    }

    /// A type expression, whose length the type grammar decides.
    fn read_type(&mut self, out: &mut Vec<DocItem>) {
        self.skip_spaces();
        let rest = self.rest();
        if rest.is_empty() {
            return;
        }
        let resolver = NameResolver::new("");
        let cx = TypeContext::new(&resolver);
        let flat = blank_line_stars(rest);
        let length = parse_type_prefix(&flat, &cx)
            .map(|(_, length)| length)
            .filter(|length| *length > 0)
            .unwrap_or_else(|| rest.find(char::is_whitespace).unwrap_or(rest.len()));
        let start = self.position;
        self.scan_type(start, start + length, out);
        self.position = start + length;
    }

    fn scan_type(&mut self, start: usize, end: usize, out: &mut Vec<DocItem>) {
        let bytes = self.text.as_bytes();
        let mut index = start;
        while index < end {
            let byte = bytes[index];
            match byte {
                b'\'' | b'"' => {
                    index += 1;
                    while index < end && bytes[index] != byte {
                        index += 1;
                    }
                    index += 1;
                }
                b'$' => {
                    index += 1;
                    while index < end && is_name_byte(bytes[index]) {
                        index += 1;
                    }
                }
                b'\\' | b'a'..=b'z' | b'A'..=b'Z' | b'_' | 0x80..=0xff => {
                    let name_start = index;
                    while index < end && (is_name_byte(bytes[index]) || bytes[index] == b'\\') {
                        index += 1;
                    }
                    let hyphenated = index < end
                        && bytes[index] == b'-'
                        && index + 1 < end
                        && bytes[index + 1].is_ascii_alphabetic();
                    if hyphenated {
                        while index < end && (is_name_byte(bytes[index]) || bytes[index] == b'-') {
                            index += 1;
                        }
                        continue;
                    }
                    let word = &self.text[name_start..index];
                    let after = self.text[index..end].trim_start();
                    let is_key = (after.starts_with(':') && !after.starts_with("::")) || after.starts_with("?:");
                    let in_shape = self.text[start..name_start].contains('{');
                    if is_key && in_shape {
                        continue;
                    }
                    let lower = word.to_ascii_lowercase();
                    let keyword = KEYWORDS.contains(&lower.as_str());
                    let template = self.templates.iter().any(|template| template == word);
                    if template && !keyword {
                        out.push(DocItem {
                            start: self.base + name_start as u32,
                            end: self.base + index as u32,
                            kind: DocItemKind::Template(word.to_string()),
                        });
                    } else if !keyword {
                        out.push(DocItem {
                            start: self.base + name_start as u32,
                            end: self.base + index as u32,
                            kind: DocItemKind::Class(word.to_string()),
                        });
                    }
                    if self.text[index..end].starts_with("::") {
                        index += 2;
                        while index < end && (is_name_byte(bytes[index]) || bytes[index] == b'*') {
                            index += 1;
                        }
                    }
                }
                _ => index += 1,
            }
        }
    }

    fn read_variable(&mut self, out: &mut Vec<DocItem>, declares_property: bool) {
        self.skip_spaces();
        let rest = self.rest();
        let prefix = rest.find(|c: char| !matches!(c, '&' | '.')).unwrap_or(rest.len());
        let after_prefix = &rest[prefix..];
        let Some(variable) = after_prefix.strip_prefix('$') else {
            return;
        };
        let name_end = variable
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(variable.len());
        if name_end == 0 {
            return;
        }
        let start = self.position + prefix;
        let name = &variable[..name_end];
        out.push(DocItem {
            start: self.base + start as u32,
            end: self.base + (start + 1 + name_end) as u32,
            kind: if declares_property {
                DocItemKind::PropertyDecl(name.to_string())
            } else {
                DocItemKind::Variable(name.to_string())
            },
        });
        self.position = start + 1 + name_end;
    }

    /// `[static] [Type] name(params)`.
    fn read_method(&mut self, out: &mut Vec<DocItem>) {
        self.skip_spaces();
        let rest = self.rest();
        let Some(open) = rest.find('(') else {
            return;
        };
        let head = &rest[..open];
        let name_start = head
            .char_indices()
            .rev()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map_or(0, |(at, c)| at + c.len_utf8());
        let name = head[name_start..].to_string();
        let static_keyword = head.starts_with("static") && head[6..].starts_with(char::is_whitespace);
        if name.is_empty() {
            return;
        }
        let begin = self.position;
        let type_start = if static_keyword { begin + 6 } else { begin };
        if type_start < begin + name_start {
            self.position = type_start;
            self.end_limited(begin + name_start, |cursor| cursor.read_type(out));
        }
        out.push(DocItem {
            start: self.base + (begin + name_start) as u32,
            end: self.base + (begin + name_start + name.len()) as u32,
            kind: DocItemKind::MethodDecl(name),
        });
        let params_start = begin + open + 1;
        let close = self.text[params_start..self.end]
            .rfind(')')
            .map_or(self.end, |at| params_start + at);
        let mut depth = 0i32;
        let mut piece_start = params_start;
        let mut pieces = Vec::new();
        for (offset, character) in self.text[params_start..close].char_indices() {
            match character {
                '<' | '(' | '{' | '[' => depth += 1,
                '>' | ')' | '}' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    pieces.push((piece_start, params_start + offset));
                    piece_start = params_start + offset + 1;
                }
                _ => {}
            }
        }
        pieces.push((piece_start, close));
        for (from, to) in pieces {
            self.position = from;
            self.end_limited(to, |cursor| {
                cursor.skip_spaces();
                if !cursor.rest().starts_with(['$', '&', '.']) {
                    cursor.read_type(out);
                }
            });
        }
        self.position = self.end;
    }

    fn end_limited(&mut self, limit: usize, run: impl FnOnce(&mut Self)) {
        let saved = self.end;
        self.end = limit.min(saved);
        run(self);
        self.end = saved;
    }

    /// The target of `@see` and `@link`.
    fn read_reference(&mut self, out: &mut Vec<DocItem>) {
        self.skip_spaces();
        let rest = self.rest();
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = rest[..word_end].trim_end_matches([',', ';', '.']);
        if word.is_empty() || word.contains("://") || word.starts_with('#') || word.starts_with('<') {
            return;
        }
        let start = self.position;
        let base = self.base;
        if let Some((class, member)) = word.split_once("::") {
            let class_range = (base + start as u32, base + (start + class.len()) as u32);
            let member_start = start + class.len() + 2;
            let (kind, name, name_start) = if let Some(property) = member.strip_prefix('$') {
                (MemberKind::Property, property, member_start + 1)
            } else if let Some(method) = member.strip_suffix("()") {
                (MemberKind::Method, method, member_start)
            } else {
                (MemberKind::Constant, member, member_start)
            };
            if !class.is_empty() && !name.is_empty() {
                out.push(DocItem {
                    start: base + name_start as u32,
                    end: base + (name_start + name.len()) as u32,
                    kind: DocItemKind::Member {
                        class: class.to_string(),
                        name: name.to_string(),
                        kind,
                        class_range,
                    },
                });
                out.push(DocItem {
                    start: class_range.0,
                    end: class_range.1,
                    kind: DocItemKind::Class(class.to_string()),
                });
            }
        } else if let Some(function) = word.strip_suffix("()") {
            if !function.is_empty() {
                out.push(DocItem {
                    start: base + start as u32,
                    end: base + (start + function.len()) as u32,
                    kind: DocItemKind::Function(function.to_string()),
                });
            }
        } else if word
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '\\' || c == '_')
            && word.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '\\')
        {
            out.push(DocItem {
                start: base + start as u32,
                end: base + (start + word.len()) as u32,
                kind: DocItemKind::Class(word.to_string()),
            });
        }
        self.position = start + word_end;
    }
}

/// The text with the ` * ` that starts each line of a doc comment turned into spaces of the same
/// length, so a type that spans lines reads as one the type grammar knows.
fn blank_line_stars(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (number, line) in text.split_inclusive('\n').enumerate() {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let star = number > 0 && line[indent..].starts_with('*') && !line[indent..].starts_with("*/");
        if star {
            out.push_str(&" ".repeat(indent + 1));
            out.push_str(&line[indent + 1..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(String, DocItemKind)> {
        doc_items(text, 0)
            .into_iter()
            .map(|item| (text[item.start as usize..item.end as usize].to_string(), item.kind))
            .collect()
    }

    fn classes(text: &str) -> Vec<String> {
        kinds(text)
            .into_iter()
            .filter_map(|(written, kind)| matches!(kind, DocItemKind::Class(_)).then_some(written))
            .collect()
    }

    #[test]
    fn finds_classes_in_types_and_skips_keywords_and_shape_keys() {
        let text = "/**\n * @param array<int, Foo\\Bar>|null $items The items\n * @return array{name: string, user: User}|\\Baz\n */";
        assert_eq!(classes(text), vec!["Foo\\Bar", "User", "\\Baz"]);
    }

    #[test]
    fn reads_variables_properties_and_methods() {
        let text = "/**\n * @property-read Foo $name\n * @method static Bar make(Baz $a, int ...$rest)\n * @param Foo &$x\n */";
        let items = kinds(text);
        assert!(items.contains(&("$name".to_string(), DocItemKind::PropertyDecl("name".to_string()))));
        assert!(items.contains(&("make".to_string(), DocItemKind::MethodDecl("make".to_string()))));
        assert!(items.contains(&("$x".to_string(), DocItemKind::Variable("x".to_string()))));
        assert_eq!(classes(text), vec!["Foo", "Bar", "Baz", "Foo"]);
    }

    #[test]
    fn reads_see_targets_and_inline_tags() {
        let text = "/**\n * Uses {@see Foo::bar()} and\n * @see Foo::$prop\n * @see helper()\n * @link https://example.com\n */";
        let items = kinds(text);
        assert!(items.iter().any(|(written, kind)| written == "bar"
            && matches!(
                kind,
                DocItemKind::Member {
                    kind: MemberKind::Method,
                    ..
                }
            )));
        assert!(items.iter().any(|(written, kind)| written == "prop"
            && matches!(
                kind,
                DocItemKind::Member {
                    kind: MemberKind::Property,
                    ..
                }
            )));
        assert!(items.contains(&("helper".to_string(), DocItemKind::Function("helper".to_string()))));
        assert_eq!(classes(text), vec!["Foo", "Foo"]);
    }

    #[test]
    fn never_slices_inside_a_character() {
        let pieces = [
            "@param ",
            "@method ",
            "@see ",
            "@property ",
            "@template ",
            "{@see ",
            "}",
            "(",
            ")",
            "<",
            ">",
            "$",
            "é",
            "ü",
            "日本",
            "\\",
            "::",
            " ",
            "\n * ",
            "int",
            "Foo",
            "|",
            "&",
            "...",
            "'",
            ",",
            "{",
            "x",
            "of ",
            "array{",
            "a: ",
            "?",
        ];
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        for _ in 0..4000 {
            let mut text = String::from("/**\n * ");
            for _ in 0..24 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                text.push_str(pieces[(seed >> 33) as usize % pieces.len()]);
            }
            for item in doc_items(&text, 7) {
                assert!(
                    text.is_char_boundary(item.start as usize - 7) && text.is_char_boundary(item.end as usize - 7),
                    "{text:?}"
                );
                assert!(item.start <= item.end);
            }
        }
    }

    #[test]
    fn template_names_are_not_classes() {
        let text = "/**\n * @template T of Model\n * @param class-string<T> $class\n * @return T\n */";
        assert_eq!(classes(text), vec!["Model"]);
    }

    #[test]
    fn finds_classes_in_shapes_that_span_lines() {
        let text = "/**\n * @return array<array{\n *     ranking: int,\n *     buyer: Buyer,\n *     other: ?Other\n * }>\n */";
        assert_eq!(classes(text), ["Buyer", "Other"]);
        let one = "/** @return array<array{ ranking: int, buyer: Buyer, other: ?Other }> */";
        assert_eq!(classes(one), ["Buyer", "Other"]);
    }
}
