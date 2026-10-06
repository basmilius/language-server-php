//! The pieces of a Blade template, read in the order the compiler reads them: comments, `@verbatim`
//! and `@php` blocks, component tags, directives and echoes. What is none of them is markup, which
//! nothing here looks at.

/// A piece of a template, with offsets in the template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    /// `{{ }}`, `{{{ }}}` or `{!! !!}`: the code between the braces.
    Echo {
        start: u32,
        end: u32,
    },
    /// `<?php ... ?>` or `<?= ... ?>` left in the template, which runs as written.
    Php {
        start: u32,
        end: u32,
        echo: bool,
    },
    /// `@php ... @endphp`: the code between the two, and where the block starts and ends.
    PhpBlock {
        at: u32,
        start: u32,
        end: u32,
        closed: bool,
    },
    Directive(Directive),
    Tag(Tag),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    /// The `@`.
    pub at: u32,
    /// The name as written, without the `@`.
    pub name: String,
    pub name_end: u32,
    /// The text between the parentheses.
    pub args: Option<(u32, u32)>,
    /// After the `)`, or after the name.
    pub end: u32,
}

impl Directive {
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }
}

/// `<x-name ...>`, `<x-name ... />` or `</x-name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub start: u32,
    pub closing: bool,
    /// What follows `x-`: `alert`, `forms.input`, `slot:title`, `mail::button`.
    pub name: String,
    pub name_start: u32,
    pub name_end: u32,
    pub attributes: Vec<Attribute>,
    pub self_closing: bool,
    /// The `>` that ends the tag is written.
    pub terminated: bool,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    pub name_start: u32,
    pub name_end: u32,
    /// `:name="$expr"`: the value is PHP.
    pub bound: bool,
    /// The text inside the quotes, or a value written without them.
    pub value: Option<(u32, u32)>,
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn find_from(text: &str, from: usize, needle: &str) -> Option<usize> {
    text.get(from..)?.find(needle).map(|found| from + found)
}

/// Where the parenthesis that closes the one at `open` is, quotes not counting.
pub fn closing_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut quote: Option<u8> = None;
    let mut position = open;
    while position < bytes.len() {
        let byte = bytes[position];
        match quote {
            Some(mark) => {
                if byte == b'\\' {
                    position += 1;
                } else if byte == mark {
                    quote = None;
                }
            }
            None => match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' => depth += 1,
                b')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Some(position);
                    }
                }
                _ => {}
            },
        }
        position += 1;
    }
    None
}

/// The pieces of a template in the order they are written.
pub fn scan(text: &str) -> Vec<Node> {
    let mut out = Vec::new();
    let mut position = 0;
    scan_range(text, &mut position, text.len(), &mut out);
    out
}

/// Reads up to `limit`.
fn scan_range(text: &str, position: &mut usize, limit: usize, out: &mut Vec<Node>) {
    let bytes = text.as_bytes();
    while *position < limit {
        let at = *position;
        if !text.is_char_boundary(at) {
            *position += 1;
            continue;
        }
        let rest = &text[at..limit];
        if rest.starts_with("{{--") {
            *position = find_from(text, at + 4, "--}}")
                .map_or(text.len(), |end| end + 4)
                .min(limit);
        } else if rest.starts_with("@{{") {
            *position = find_from(text, at + 3, "}}").map_or(limit, |end| end + 2).min(limit);
        } else if let Some(after) = echo(text, at, limit, out) {
            *position = after;
        } else if rest.starts_with("<?php") || rest.starts_with("<?=") {
            let echo = rest.starts_with("<?=");
            let start = at + if echo { 3 } else { 5 };
            let end = find_from(text, start, "?>").unwrap_or(text.len());
            out.push(Node::Php {
                start: start as u32,
                end: end as u32,
                echo,
            });
            *position = (end + 2).min(text.len());
        } else if rest.starts_with("<x-") || rest.starts_with("</x-") {
            *position = tag(text, at, out);
        } else if bytes[at] == b'@' && (at == 0 || !is_word(bytes[at - 1])) {
            *position = directive(text, at, limit, out);
        } else {
            *position += 1;
        }
    }
}

/// Reads the echo at a position, if one starts there, and returns where reading goes on.
fn echo(text: &str, at: usize, limit: usize, out: &mut Vec<Node>) -> Option<usize> {
    let rest = &text[at..limit];
    let open = ["{!!", "{{{", "{{"].into_iter().find(|open| rest.starts_with(open))?;
    let close = match open {
        "{!!" => "!!}",
        "{{{" => "}}}",
        _ => "}}",
    };
    let start = at + open.len();
    let end = find_from(text, start, close)
        .filter(|end| *end <= limit)
        .unwrap_or(limit);
    out.push(Node::Echo {
        start: start as u32,
        end: end as u32,
    });
    Some((end + close.len()).min(limit))
}

fn directive(text: &str, at: usize, limit: usize, out: &mut Vec<Node>) -> usize {
    let bytes = text.as_bytes();
    if bytes.get(at + 1) == Some(&b'@') {
        let escaped = text[at + 2..limit].bytes().take_while(|byte| is_word(*byte)).count();
        return at + 2 + escaped;
    }
    let mut name_end = at + 1 + text[at + 1..limit].bytes().take_while(|byte| is_word(*byte)).count();
    if name_end == at + 1 {
        return at + 1;
    }
    if text[name_end..limit].starts_with("::") {
        let more = text[name_end + 2..limit]
            .bytes()
            .take_while(|byte| is_word(*byte))
            .count();
        if more > 0 {
            name_end += 2 + more;
        }
    }
    let name = &text[at + 1..name_end];
    if name.eq_ignore_ascii_case("verbatim") {
        return find_ignoring_case(text, name_end, "@endverbatim").map_or(text.len(), |end| end + "@endverbatim".len());
    }
    let spaces = text[name_end..limit]
        .bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count();
    let open = name_end + spaces;
    let has_args = bytes.get(open) == Some(&b'(') && open < limit;
    if name.eq_ignore_ascii_case("php") && !has_args {
        let found = find_ignoring_case(text, name_end, "@endphp");
        let end = found.unwrap_or(text.len());
        out.push(Node::PhpBlock {
            at: at as u32,
            start: name_end as u32,
            end: end as u32,
            closed: found.is_some(),
        });
        return found.map_or(text.len(), |end| end + "@endphp".len());
    }
    let close = if has_args {
        closing_paren(text, open).filter(|close| *close < limit)
    } else {
        None
    };
    let (args, end) = match close {
        Some(close) => (Some(((open + 1) as u32, close as u32)), close + 1),
        None => (None, name_end),
    };
    out.push(Node::Directive(Directive {
        at: at as u32,
        name: name.to_string(),
        name_end: name_end as u32,
        args,
        end: end as u32,
    }));
    end
}

fn find_ignoring_case(text: &str, from: usize, needle: &str) -> Option<usize> {
    let haystack = text.get(from..)?.to_ascii_lowercase();
    haystack.find(needle).map(|found| from + found)
}

fn is_tag_name_byte(byte: u8) -> bool {
    is_word(byte) || matches!(byte, b'-' | b'.' | b':')
}

fn is_attribute_byte(byte: u8) -> bool {
    is_word(byte) || matches!(byte, b'-' | b'.' | b':' | b'@' | b'%')
}

fn tag(text: &str, at: usize, out: &mut Vec<Node>) -> usize {
    let bytes = text.as_bytes();
    let closing = text[at..].starts_with("</");
    let name_start = at + if closing { 4 } else { 3 };
    let name_length = text[name_start..]
        .bytes()
        .take_while(|byte| is_tag_name_byte(*byte))
        .count();
    let name_end = name_start + name_length;
    let mut tag = Tag {
        start: at as u32,
        closing,
        name: text[name_start..name_end].to_string(),
        name_start: name_start as u32,
        name_end: name_end as u32,
        attributes: Vec::new(),
        self_closing: false,
        terminated: false,
        end: name_end as u32,
    };
    let mut position = name_end;
    let mut inner = Vec::new();
    while position < bytes.len() {
        let byte = bytes[position];
        if byte.is_ascii_whitespace() {
            position += 1;
        } else if byte == b'>' {
            tag.terminated = true;
            position += 1;
            break;
        } else if text[position..].starts_with("/>") {
            tag.self_closing = true;
            tag.terminated = true;
            position += 2;
            break;
        } else if let Some(after) = echo(text, position, text.len(), &mut inner) {
            position = after;
        } else if byte == b'@' && !is_attribute_name_at(text, position) {
            position = directive(text, position, text.len(), &mut inner).max(position + 1);
        } else if is_attribute_byte(byte) {
            let start = position;
            position += text[position..]
                .bytes()
                .take_while(|byte| is_attribute_byte(*byte))
                .count();
            let written = &text[start..position];
            let bound = written.starts_with(':') && !written.starts_with("::");
            let mut attribute = Attribute {
                name: written.trim_start_matches(':').to_string(),
                name_start: (start + usize::from(bound)) as u32,
                name_end: position as u32,
                bound,
                value: None,
            };
            if bytes.get(position) == Some(&b'=') {
                position += 1;
                match bytes.get(position) {
                    Some(&quote @ (b'"' | b'\'')) => {
                        let value_start = position + 1;
                        let value_end = text[value_start..]
                            .bytes()
                            .position(|byte| byte == quote)
                            .map_or(text.len(), |found| value_start + found);
                        attribute.value = Some((value_start as u32, value_end as u32));
                        if !bound {
                            let mut cursor = value_start;
                            scan_range(text, &mut cursor, value_end, &mut inner);
                        }
                        position = (value_end + 1).min(text.len());
                    }
                    _ => {
                        let value_start = position;
                        position += text[position..]
                            .bytes()
                            .take_while(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
                            .count();
                        attribute.value = Some((value_start as u32, position as u32));
                    }
                }
            }
            tag.attributes.push(attribute);
        } else {
            position += 1;
        }
    }
    tag.end = position as u32;
    out.push(Node::Tag(tag));
    out.extend(inner);
    position
}

/// An `@` in a tag starts a directive (`@class([...])`), unless it is the start of an attribute
/// name such as Alpine's `@click`.
fn is_attribute_name_at(text: &str, at: usize) -> bool {
    let rest = &text[at + 1..];
    let name = rest.bytes().take_while(|byte| is_attribute_byte(*byte)).count();
    let after = rest.as_bytes().get(name).copied();
    after == Some(b'=') || rest[..name].contains(['.', ':'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<String> {
        scan(text)
            .into_iter()
            .map(|node| match node {
                Node::Echo { start, end } => format!("echo `{}`", &text[start as usize..end as usize]),
                Node::Php { start, end, .. } => format!("php `{}`", &text[start as usize..end as usize]),
                Node::PhpBlock { start, end, .. } => format!("block `{}`", &text[start as usize..end as usize]),
                Node::Directive(directive) => match directive.args {
                    Some((start, end)) => format!("@{}({})", directive.name, &text[start as usize..end as usize]),
                    None => format!("@{}", directive.name),
                },
                Node::Tag(tag) => {
                    let attributes: Vec<String> = tag
                        .attributes
                        .iter()
                        .map(|attribute| {
                            let value = attribute
                                .value
                                .map(|(start, end)| text[start as usize..end as usize].to_string())
                                .unwrap_or_default();
                            format!("{}{}={value}", if attribute.bound { ":" } else { "" }, attribute.name)
                        })
                        .collect();
                    format!(
                        "<{}x-{} {}{}>",
                        if tag.closing { "/" } else { "" },
                        tag.name,
                        attributes.join(" "),
                        if tag.self_closing { " /" } else { "" }
                    )
                }
            })
            .collect()
    }

    #[test]
    fn reads_directives_echoes_and_blocks() {
        assert_eq!(
            kinds("@if ($a) {{ $b }} @else {!! $c !!} @endif @@if mail@example.com @{{ $d }} {{-- @if --}}"),
            ["@if($a)", "echo ` $b `", "@else", "echo ` $c `", "@endif"]
        );
        assert_eq!(
            kinds("@php $a = 1; @endphp @php($b = 2) @verbatim {{ $c }} @if @endverbatim <?php echo $d; ?>"),
            ["block ` $a = 1; `", "@php($b = 2)", "php ` echo $d; `"]
        );
        assert_eq!(
            kinds("@foreach ($xs as $x)\n@endforeach"),
            ["@foreach($xs as $x)", "@endforeach"]
        );
        assert_eq!(kinds("@section('a', ')')"), ["@section('a', ')')"]);
        assert_eq!(kinds("@livewire::styles"), ["@livewire::styles"]);
    }

    #[test]
    fn reads_component_tags_and_their_attributes() {
        assert_eq!(
            kinds(
                "<x-alert type=\"error\" :message=\"$m\" class=\"a {{ $c }}\" @click=\"go\" {{ $attributes }} @class(['a']) />"
            ),
            [
                "<x-alert type=error :message=$m class=a {{ $c }} @click=go />",
                "echo ` $c `",
                "echo ` $attributes `",
                "@class(['a'])"
            ]
        );
        assert_eq!(
            kinds("<x-slot:title>Hi</x-slot><x-forms.input name=x></x-forms.input>"),
            [
                "<x-slot:title >",
                "</x-slot >",
                "<x-forms.input name=x>",
                "</x-forms.input >"
            ]
        );
    }

    #[test]
    fn every_prefix_reads() {
        let text = "<x-a :b=\"$c\">@if ($d) {{ $e }} @endif @php $f @endphp {!! é !!}</x-a> @{{ x }} {{-- y";
        for end in (0..=text.len()).filter(|end| text.is_char_boundary(*end)) {
            let _ = scan(&text[..end]);
        }
    }
}
