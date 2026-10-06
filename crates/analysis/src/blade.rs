//! A Blade template, read just far enough to follow what a person points at: the views, translations
//! and components its directives and tags name, and the PHP in `{{ }}`, `@php` and the arguments of a
//! directive, each read as a small document of its own. There is no model of the template as a whole:
//! the variables of the controller are not known, and what a directive does to the scope is not
//! followed. A template that is not read as PHP gets nothing but this.

use php_index::Index;
use php_index::framework::keys::{KeyKind, definitions};
use php_index::framework::overlay::{Marker, directive_markers};
use php_syntax::SyntaxKind::*;
use php_syntax::parse;

use php_syntax::TextRange;

use crate::ast::range_of;
use crate::completion::{CompletionList, CompletionOptions, complete};
use crate::context::FileContext;
use crate::frameworks::complete::key_items;
use crate::infer::Analyzer;
use crate::nav::{HoverResult, Place, hover_markdown};
use crate::refs::{Access, Hit, HitKind, Query, Symbol, hits_in_file};

/// What a stretch of the template is, in the offsets of the template.
#[derive(Debug, PartialEq)]
enum Piece {
    /// A name a directive or a tag gives: a view, a translation, a component.
    Name {
        kind: KeyKind,
        value: String,
        start: u32,
        end: u32,
    },
    /// PHP code, with what turns it into a statement.
    Php {
        start: u32,
        end: u32,
        prefix: &'static str,
        code: String,
        suffix: &'static str,
    },
}

const OPEN: &str = "<?php ";

impl Piece {
    fn contains(&self, offset: u32) -> bool {
        let (start, end) = match self {
            Piece::Name { start, end, .. } | Piece::Php { start, end, .. } => (*start, *end),
        };
        start <= offset && offset <= end
    }
}

/// A piece of PHP as a document: where the code starts in it, and its text.
struct Mini {
    text: String,
    code_start: u32,
    blade_start: u32,
}

impl Mini {
    fn of(piece: &Piece) -> Option<Mini> {
        let Piece::Php {
            start,
            prefix,
            code,
            suffix,
            ..
        } = piece
        else {
            return None;
        };
        Some(Mini {
            text: format!("{OPEN}{prefix}{code}{suffix}"),
            code_start: (OPEN.len() + prefix.len()) as u32,
            blade_start: *start,
        })
    }

    fn to_mini(&self, offset: u32) -> u32 {
        offset - self.blade_start + self.code_start
    }

    fn to_blade(&self, offset: u32) -> u32 {
        offset.saturating_sub(self.code_start) + self.blade_start
    }

    fn code_end(&self, piece_len: u32) -> u32 {
        self.code_start + piece_len
    }
}

/// Where the arguments of a directive end: the matching parenthesis, which quotes do not count.
fn closing_paren(text: &str, open: usize) -> Option<usize> {
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

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':')
}

fn pieces(index: &Index, text: &str) -> Vec<Piece> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut position = 0;
    while position < bytes.len() {
        if !text.is_char_boundary(position) {
            position += 1;
            continue;
        }
        let rest = &text[position..];
        if rest.starts_with("{{--") {
            position += rest.find("--}}").map_or(rest.len(), |end| end + 4);
        } else if let Some(echo) = ["{!!", "{{{", "{{"].iter().find(|open| rest.starts_with(**open)) {
            if position > 0 && bytes[position - 1] == b'@' {
                position += echo.len();
                continue;
            }
            let close = match *echo {
                "{!!" => "!!}",
                "{{{" => "}}}",
                _ => "}}",
            };
            let from = position + echo.len();
            let end = text[from..].find(close).map_or(text.len(), |at| from + at);
            out.push(Piece::Php {
                start: from as u32,
                end: end as u32,
                prefix: "",
                code: text[from..end].to_string(),
                suffix: ";",
            });
            position = (end + close.len()).min(text.len());
        } else if rest.starts_with("<x-") || rest.starts_with("</x-") {
            let from = position + rest.find("x-").map_or(0, |at| at + 2);
            let length = text[from..].bytes().take_while(|byte| is_name_byte(*byte)).count();
            let name = &text[from..from + length];
            if !name.starts_with("slot") && name != "dynamic-component" && !name.is_empty() {
                out.push(Piece::Name {
                    kind: KeyKind::Component,
                    value: name.to_string(),
                    start: from as u32,
                    end: (from + length) as u32,
                });
            }
            position = from + length.max(1);
        } else if bytes[position] == b'@'
            && (position == 0 || !(bytes[position - 1].is_ascii_alphanumeric() || bytes[position - 1] == b'@'))
        {
            position = directive(index, text, position, &mut out);
        } else {
            position += 1;
        }
    }
    out
}

/// Reads the directive at a position, adds what it holds to `out` and returns where reading goes on.
fn directive(index: &Index, text: &str, at: usize, out: &mut Vec<Piece>) -> usize {
    let bytes = text.as_bytes();
    let name_length = text[at + 1..]
        .bytes()
        .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        .count();
    if name_length == 0 {
        return at + 1;
    }
    let name = &text[at + 1..at + 1 + name_length];
    let after = at + 1 + name_length;
    if name.eq_ignore_ascii_case("php") && !text[after..].trim_start_matches([' ', '\t']).starts_with('(') {
        let end = text[after..].find("@endphp").map_or(text.len(), |found| after + found);
        out.push(Piece::Php {
            start: after as u32,
            end: end as u32,
            prefix: "",
            code: text[after..end].to_string(),
            suffix: "",
        });
        return end;
    }
    let spaces = text[after..]
        .bytes()
        .take_while(|byte| *byte == b' ' || *byte == b'\t')
        .count();
    let open = after + spaces;
    if bytes.get(open) != Some(&b'(') {
        return after;
    }
    let Some(close) = closing_paren(text, open) else {
        return after;
    };
    let args_start = open + 1;
    let args = &text[args_start..close];
    for marker in directive_markers(index, name) {
        let Marker::Key { kind, position, .. } = marker else {
            continue;
        };
        let Some(kind) = KeyKind::parse(&kind) else {
            continue;
        };
        if let Some((value, start, end)) = nth_string(args, position) {
            out.push(Piece::Name {
                kind,
                value,
                start: (args_start + start) as u32,
                end: (args_start + end) as u32,
            });
        }
    }
    let (prefix, suffix) = match name.to_ascii_lowercase().as_str() {
        "foreach" | "forelse" => ("foreach (", ") {}"),
        "for" => ("for (", ") {}"),
        "php" => ("", ";"),
        _ => ("[", "];"),
    };
    out.push(Piece::Php {
        start: args_start as u32,
        end: close as u32,
        prefix,
        code: args.to_string(),
        suffix,
    });
    close + 1
}

/// The string literal that is the n-th argument of an argument list written out, with where its text
/// starts and ends in the list.
fn nth_string(args: &str, position: usize) -> Option<(String, usize, usize)> {
    let wrapped = format!("<?php [{args}];");
    let tree = parse(&wrapped).syntax();
    let array = tree.descendants().find(|node| node.kind() == ARRAY_EXPR)?;
    let item = array
        .children()
        .filter(|node| node.kind() == ARRAY_ITEM)
        .nth(position)?;
    let literal = item.children().last()?;
    let (value, span) = php_index::test_facts::string_value(&literal)?;
    let prefix = "<?php [".len();
    Some((value, span.start as usize - prefix, span.end as usize - prefix))
}

fn piece_at(index: &Index, text: &str, offset: u32) -> Option<Piece> {
    let found = pieces(index, text);
    let (names, php): (Vec<Piece>, Vec<Piece>) =
        found.into_iter().partition(|piece| matches!(piece, Piece::Name { .. }));
    names.into_iter().chain(php).find(|piece| piece.contains(offset))
}

fn shift_place(mini: &Mini, place: Place) -> Place {
    match place.path {
        Some(_) => place,
        None => Place {
            path: None,
            span: php_index::Span {
                start: mini.to_blade(place.span.start),
                end: mini.to_blade(place.span.end),
            },
        },
    }
}

/// Whether a file is a Blade template, by its name.
pub fn is_template(path: &std::path::Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".blade.php"))
}

/// The name under an offset of a template, as a directive, a tag or a string of the PHP in it gives it.
pub fn key_symbol_at(index: &Index, text: &str, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    match piece_at(index, text, offset)? {
        Piece::Name {
            kind,
            value,
            start,
            end,
        } => Some((
            range_of(start, end),
            vec![Symbol::Key {
                kind,
                name: value,
                scope: None,
            }],
        )),
        piece @ Piece::Php { .. } => {
            let mini = Mini::of(&piece)?;
            let root = parse(&mini.text).syntax();
            let at = mini.to_mini(offset);
            let key = crate::frameworks::keys::key_at(&Analyzer::new(index, &root, at), at)?;
            Some((
                range_of(
                    mini.to_blade(u32::from(key.range.start())),
                    mini.to_blade(u32::from(key.range.end())),
                ),
                vec![Symbol::Key {
                    kind: key.kind,
                    name: key.value,
                    scope: key.scope,
                }],
            ))
        }
    }
}

/// The places of a template that name a route, a view, a translation or the like.
pub fn key_hits(index: &Index, text: &str, query: &Query) -> Vec<Hit> {
    let Symbol::Key { name, .. } = &query.symbol else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for piece in pieces(index, text) {
        match &piece {
            Piece::Name {
                kind,
                value,
                start,
                end,
            } => {
                let symbol = Symbol::Key {
                    kind: *kind,
                    name: value.clone(),
                    scope: None,
                };
                if query.matches(&symbol) {
                    hits.push(Hit {
                        range: range_of(*start, *end),
                        kind: HitKind::Reference,
                        access: Access::Read,
                        dollar: false,
                        via_alias: false,
                        symbol,
                    });
                }
            }
            Piece::Php { code, .. } => {
                if !code.contains(name.as_str()) {
                    continue;
                }
                let Some(mini) = Mini::of(&piece) else {
                    continue;
                };
                let root = parse(&mini.text).syntax();
                let ctx = FileContext::new(index, &root);
                for mut hit in hits_in_file(&ctx, &mini.text, query) {
                    hit.range = range_of(
                        mini.to_blade(u32::from(hit.range.start())),
                        mini.to_blade(u32::from(hit.range.end())),
                    );
                    hits.push(hit);
                }
            }
        }
    }
    hits.sort_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits.dedup_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits
}

/// Where the name or the PHP under an offset of a template is declared.
pub fn definitions_at(index: &Index, text: &str, offset: u32) -> Vec<Place> {
    match piece_at(index, text, offset) {
        Some(Piece::Name { kind, value, .. }) => definitions(index, kind, &value, None)
            .into_iter()
            .map(|definition| Place {
                path: Some(definition.path),
                span: definition.span,
            })
            .collect(),
        Some(piece @ Piece::Php { .. }) => {
            let Some(mini) = Mini::of(&piece) else {
                return Vec::new();
            };
            let root = parse(&mini.text).syntax();
            let at = mini.to_mini(offset);
            Analyzer::new(index, &root, at)
                .definitions(at)
                .into_iter()
                .map(|place| shift_place(&mini, place))
                .collect()
        }
        None => Vec::new(),
    }
}

/// What the name or the PHP under an offset is.
pub fn hover_at(index: &Index, text: &str, offset: u32) -> Option<HoverResult> {
    match piece_at(index, text, offset)? {
        Piece::Name {
            kind,
            value,
            start,
            end,
        } => {
            let sections: Vec<String> = crate::frameworks::keys::describe(index, kind, &value, None)
                .iter()
                .map(hover_markdown)
                .collect();
            Some(HoverResult {
                markdown: sections.join("\n\n---\n\n"),
                range: range_of(start, end),
            })
        }
        piece @ Piece::Php { .. } => {
            let mini = Mini::of(&piece)?;
            let root = parse(&mini.text).syntax();
            let at = mini.to_mini(offset);
            let found = Analyzer::new(index, &root, at).hover(at)?;
            Some(HoverResult {
                markdown: found.markdown,
                range: range_of(
                    mini.to_blade(u32::from(found.range.start())),
                    mini.to_blade(u32::from(found.range.end())),
                ),
            })
        }
    }
}

/// What can be typed at an offset of a template.
pub fn complete_at(index: &Index, text: &str, offset: u32, options: CompletionOptions) -> Option<CompletionList> {
    match piece_at(index, text, offset)? {
        Piece::Name { kind, start, end, .. } => {
            let typed = text.get(start as usize..offset as usize)?;
            Some(key_items(index, kind, None, typed, (start, end), options))
        }
        piece @ Piece::Php { end, start, .. } => {
            let mini = Mini::of(&piece)?;
            let at = mini.to_mini(offset);
            let mut list = complete(index, &mini.text, at, options);
            let limit = mini.code_end(end - start);
            list.items
                .retain(|item| item.edit.start >= mini.code_start && item.edit.end <= limit);
            for item in &mut list.items {
                item.edit.start = mini.to_blade(item.edit.start);
                item.edit.end = mini.to_blade(item.edit.end);
                item.additional_edits.clear();
            }
            Some(list)
        }
    }
}

#[cfg(test)]
mod tests {
    use php_index::framework::testing::HELPERS;

    use super::*;
    use crate::testing::{CURSOR, Fixture};

    fn fixture() -> Fixture {
        let mut files = HELPERS.to_vec();
        files.extend_from_slice(&[
            ("resources/views/welcome.blade.php", "x"),
            ("resources/views/layouts/app.blade.php", "x"),
            ("resources/views/components/alert.blade.php", "x"),
            ("lang/en/messages.php", "<?php return ['welcome' => 'Welcome'];"),
            ("config/app.php", "<?php return ['name' => 'x'];"),
            (
                "app/Models/User.php",
                "<?php namespace App\\Models; class User { public static function count(): int {} public function name(): string {} }",
            ),
        ]);
        Fixture::framework(&files)
    }

    fn places(template: &str) -> Vec<String> {
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        let text = template.replacen(CURSOR, "", 1);
        definitions_at(&fixture().index, &text, offset)
            .into_iter()
            .map(|place| {
                place
                    .path
                    .map(|path| {
                        path.strip_prefix("/project")
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .into_owned()
                    })
                    .unwrap_or_else(|| format!("{}..{}", place.span.start, place.span.end))
            })
            .collect()
    }

    fn completions(template: &str) -> Vec<String> {
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        let text = template.replacen(CURSOR, "", 1);
        complete_at(&fixture().index, &text, offset, CompletionOptions::default())
            .map(|list| list.items.into_iter().map(|item| item.label).collect())
            .unwrap_or_default()
    }

    #[test]
    fn directives_name_views_and_translations() {
        assert_eq!(
            places("@extends('layouts.a$0pp')\n"),
            ["resources/views/layouts/app.blade.php"]
        );
        assert_eq!(
            places("<div>@include('wel$0come', ['a' => 1])</div>"),
            ["resources/views/welcome.blade.php"]
        );
        assert_eq!(
            places("@includeWhen($x, 'wel$0come')"),
            ["resources/views/welcome.blade.php"]
        );
        assert_eq!(places("@lang('messages.wel$0come')"), ["lang/en/messages.php"]);
    }

    #[test]
    fn a_component_tag_leads_to_its_template() {
        assert_eq!(
            places("<x-al$0ert type=\"error\" />"),
            ["resources/views/components/alert.blade.php"]
        );
        assert_eq!(
            places("<div></x-al$0ert>"),
            ["resources/views/components/alert.blade.php"]
        );
        assert!(places("<x-slot:ti$0tle>").is_empty());
    }

    #[test]
    fn php_in_the_template_is_followed() {
        assert_eq!(
            places("<p>{{ route('x') }}{{ config('app.na$0me') }}</p>"),
            ["config/app.php"]
        );
        assert_eq!(places("{!! __('messages.wel$0come') !!}"), ["lang/en/messages.php"]);
        assert_eq!(
            places("@php $total = \\App\\Models\\Us$0er::count(); @endphp"),
            ["app/Models/User.php"]
        );
        assert_eq!(
            places("@if (\\App\\Models\\Us$0er::count() > 1) x @endif"),
            ["app/Models/User.php"]
        );
        assert!(places("{{-- {{ config('app.na$0me') }} --}}").is_empty());
        assert!(places("<p>plain wor$0ds</p>").is_empty());
    }

    #[test]
    fn completes_names_and_php() {
        assert_eq!(completions("@include('wel$0')"), ["welcome"]);
        assert_eq!(completions("<x-al$0"), ["alert"]);
        assert_eq!(completions("<x-al$0 />"), ["alert"]);
        assert_eq!(completions("{{ config('app.$0') }}"), ["app.name"]);
        assert!(completions("{{ \\App\\Models\\User::co$0 }}").contains(&"count".to_string()));
    }

    #[test]
    fn a_template_in_words_with_accents_is_read() {
        assert_eq!(
            places("<p>Dé prijs: {{ config('app.na$0me') }} €5</p>"),
            ["config/app.php"]
        );
        assert_eq!(
            places("<p>één</p> @include('wel$0come')"),
            ["resources/views/welcome.blade.php"]
        );
    }

    #[test]
    fn no_prefix_of_a_template_breaks_the_reading() {
        let template = "@extends('layouts.app')\n@section('c')\n<x-alert type=\"é\">{{ $a->b(config('app.name'), \"x\") }}{!! __('m.w') !!}</x-alert>\n@foreach ($xs as $x) @if ($x->y) {{-- c --}} @endif @endforeach\n@php $t = \\App\\Models\\User::count(); @endphp @can('x', $y) {{ é }} @endcan @@x {{{ $z }}}";
        let fixture = fixture();
        for end in (0..=template.len()).filter(|end| template.is_char_boundary(*end)) {
            let text = &template[..end];
            for offset in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
                let _ = definitions_at(&fixture.index, text, offset as u32);
                let _ = hover_at(&fixture.index, text, offset as u32);
                let _ = complete_at(&fixture.index, text, offset as u32, CompletionOptions::default());
            }
        }
    }

    #[test]
    fn hovers_the_php_in_a_template() {
        let template = "{{ \\App\\Models\\User::cou$0nt() }}";
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        let text = template.replacen(CURSOR, "", 1);
        let hover = hover_at(&fixture().index, &text, offset).expect("a hover");
        assert!(hover.markdown.contains("count"), "{}", hover.markdown);
        assert_eq!(
            &text[usize::from(hover.range.start())..usize::from(hover.range.end())],
            "count"
        );
    }
}
