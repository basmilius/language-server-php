//! What Blade templates declare for other templates to fill: the sections a layout yields
//! (`@yield('content')`, or a `@section('sidebar') ... @show` a child may replace), the stacks it
//! renders (`@stack('scripts')`), and the props and slots of an anonymous component. Read from the
//! text of the templates, without a model of them, since only the names matter here.

use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::{ARRAY_EXPR, ARRAY_ITEM};

use super::Section;
use super::views::Views;
use crate::index::Index;
use crate::model::Span;

/// A name and where a template writes it, inside its quotes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub name: String,
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Default)]
pub struct Layouts {
    pub sections: Vec<Placed>,
    pub stacks: Vec<Placed>,
}

impl Section for Layouts {
    fn build(index: &Index) -> Self {
        let mut layouts = Layouts::default();
        let views = index.section::<Views>();
        for view in &views.views {
            if !view.path.to_string_lossy().ends_with(".blade.php") {
                continue;
            }
            let Some(text) = index.read_text(&view.path) else {
                continue;
            };
            for (directive, name, span) in directives(&text) {
                let placed = Placed {
                    name,
                    path: view.path.clone(),
                    span,
                };
                match directive.as_str() {
                    "yield" => layouts.sections.push(placed),
                    "section" if ends_with_show(&text, span.end as usize) => layouts.sections.push(placed),
                    "stack" => layouts.stacks.push(placed),
                    _ => {}
                }
            }
        }
        layouts
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_below(root, path, "resources/views")
    }
}

impl Layouts {
    pub fn sections_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Placed> + 'a {
        self.sections.iter().filter(move |placed| placed.name == name)
    }

    pub fn stacks_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Placed> + 'a {
        self.stacks.iter().filter(move |placed| placed.name == name)
    }
}

fn is_word(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Each `@name('first argument'` of a template, lowercase, with the text of the string and where
/// it is.
fn directives(text: &str) -> Vec<(String, String, Span)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find('@') {
        let start = at + found;
        at = start + 1;
        if start > 0 && (is_word(bytes[start - 1]) || bytes[start - 1] == b'@') {
            continue;
        }
        let name_end = start + 1 + text[start + 1..].bytes().take_while(|byte| is_word(*byte)).count();
        let mut position = name_end
            + text[name_end..]
                .bytes()
                .take_while(|byte| *byte == b' ' || *byte == b'\t')
                .count();
        if bytes.get(position) != Some(&b'(') {
            continue;
        }
        position += 1;
        position += text[position..].bytes().take_while(u8::is_ascii_whitespace).count();
        let Some(&quote) = bytes.get(position).filter(|byte| matches!(byte, b'\'' | b'"')) else {
            continue;
        };
        let value_start = position + 1;
        let Some(length) = text[value_start..]
            .bytes()
            .position(|byte| byte == quote || byte == b'\n')
        else {
            continue;
        };
        if bytes[value_start + length] != quote {
            continue;
        }
        out.push((
            text[start + 1..name_end].to_ascii_lowercase(),
            text[value_start..value_start + length].to_string(),
            Span {
                start: value_start as u32,
                end: (value_start + length) as u32,
            },
        ));
    }
    out
}

/// Whether the section that starts before `from` ends with `@show`, which renders it in place and
/// lets a child replace it.
fn ends_with_show(text: &str, from: usize) -> bool {
    let rest = text[from..].to_ascii_lowercase();
    let ends = ["@show", "@endsection", "@stop", "@append", "@overwrite"];
    ends.iter()
        .filter_map(|end| rest.find(end).map(|at| (at, *end)))
        .min_by_key(|(at, _)| *at)
        .is_some_and(|(_, end)| end == "@show")
}

/// The directives a project registers with `Blade::directive('name', ...)`, which win over the
/// compiler's own of the same name, and with `Blade::if('name', ...)`, which open a conditional
/// block with `@else<name>`, `@unless<name>` and `@end<name>`.
#[derive(Default)]
pub struct Directives {
    pub plain: Vec<String>,
    pub conditionals: Vec<String>,
}

impl Section for Directives {
    fn build(index: &Index) -> Self {
        let mut directives = Directives::default();
        let root = index.framework_root();
        let skipped = ["resources/views", "database", "tests", "lang", "config", "storage"];
        for file in index.files() {
            if file.origin != crate::index::Origin::Project
                || skipped.iter().any(|folder| super::is_below(root, &file.path, folder))
            {
                continue;
            }
            let Some(text) = index.read_text(&file.path) else {
                continue;
            };
            if !text.contains("directive(") && !text.contains("::if(") {
                continue;
            }
            directives.scan(&text);
        }
        directives
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

impl Directives {
    fn scan(&mut self, text: &str) {
        for (call, conditional) in [
            ("Blade::directive(", false),
            ("->directive(", false),
            ("Blade::if(", true),
        ] {
            let mut rest = text;
            while let Some(at) = rest.find(call) {
                rest = &rest[at + call.len()..];
                let trimmed = rest.trim_start();
                let Some(quote) = trimmed.chars().next().filter(|quote| matches!(quote, '\'' | '"')) else {
                    continue;
                };
                let Some(length) = trimmed[1..].find(quote) else {
                    continue;
                };
                let name = trimmed[1..1 + length].to_string();
                let list = if conditional {
                    &mut self.conditionals
                } else {
                    &mut self.plain
                };
                if !name.is_empty() && !list.contains(&name) {
                    list.push(name);
                }
            }
        }
    }
}

/// The template of a component: its own file when it is anonymous, else `components.<tag>`.
pub fn component_template(index: &Index, tag: &str) -> Option<PathBuf> {
    let views = index.section::<Views>();
    views
        .components
        .iter()
        .find(|component| component.tag == tag && component.class.is_none())
        .map(|component| component.path.clone())
        .or_else(|| views.find(&format!("components.{tag}")).map(|view| view.path.clone()))
}

/// The props an anonymous component declares with `@props([...])`, with where each is written.
pub fn props_of(text: &str) -> Vec<(String, Span)> {
    let Some(at) = text.find("@props") else {
        return Vec::new();
    };
    let Some(open) = text[at..].find('(').map(|found| at + found) else {
        return Vec::new();
    };
    let Some(close) = closing_paren(text, open) else {
        return Vec::new();
    };
    let wrapped = format!("<?php {};", &text[open + 1..close]);
    let shift = |offset: u32| offset - 6 + open as u32 + 1;
    let tree = php_syntax::parse(&wrapped).syntax();
    let Some(array) = tree.descendants().find(|node| node.kind() == ARRAY_EXPR) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in array.children().filter(|node| node.kind() == ARRAY_ITEM) {
        let Some(name) = item.children().next() else {
            continue;
        };
        if let Some((value, span)) = crate::test_facts::string_value(&name) {
            out.push((
                value,
                Span {
                    start: shift(span.start),
                    end: shift(span.end),
                },
            ));
        }
    }
    out
}

fn closing_paren(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let (mut depth, mut quote, mut position) = (0usize, None::<u8>, open);
    while position < bytes.len() {
        let byte = bytes[position];
        match quote {
            Some(_) if byte == b'\\' => position += 1,
            Some(mark) if byte == mark => quote = None,
            Some(_) => {}
            None => match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
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

/// The variables a component's template uses that it does not make itself, which is what a slot or
/// an attribute can fill: `$title` of `{{ $title }}`, with where it is first written.
pub fn slots_of(text: &str) -> Vec<(String, Span)> {
    const OWN: &[&str] = &[
        "attributes",
        "slot",
        "loop",
        "errors",
        "component",
        "__env",
        "app",
        "this",
        "message",
    ];
    let bytes = text.as_bytes();
    let mut out: Vec<(String, Span)> = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find('$') {
        let start = at + found + 1;
        at = start;
        let length = text[start..].bytes().take_while(|byte| is_word(*byte)).count();
        if length == 0 || bytes[start].is_ascii_digit() {
            continue;
        }
        let name = &text[start..start + length];
        if OWN.contains(&name) || out.iter().any(|(known, _)| known == name) {
            continue;
        }
        out.push((
            name.to_string(),
            Span {
                start: start as u32,
                end: (start + length) as u32,
            },
        ));
    }
    out
}

/// `show-view-count` is the prop `showViewCount`.
pub fn attribute_key(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for character in name.trim_start_matches(':').chars() {
        if character == '-' || character == '_' {
            upper = !out.is_empty();
        } else if upper {
            out.extend(character.to_uppercase());
            upper = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// `showViewCount` is written `show-view-count`.
pub fn kebab(name: &str) -> String {
    let mut out = String::new();
    for character in name.chars() {
        if character.is_uppercase() {
            if !out.is_empty() {
                out.push('-');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn reads_sections_stacks_props_and_slots() {
        let index = project(&[
            (
                "resources/views/layouts/app.blade.php",
                "@yield('title', 'Home') @section('sidebar') x @show @section('other') y @endsection @stack(\"scripts\") user@yield('no')",
            ),
            (
                "resources/views/home.blade.php",
                "@extends('layouts.app') @section('title', 'Hi')",
            ),
        ]);
        let layouts = index.section::<Layouts>();
        let names: Vec<&str> = layouts.sections.iter().map(|placed| placed.name.as_str()).collect();
        assert_eq!(names, ["title", "sidebar"]);
        assert_eq!(layouts.stacks.len(), 1);
        assert_eq!(layouts.stacks[0].name, "scripts");

        let card = "@props(['title', 'showCount' => false])\n<div>{{ $title }} {{ $footer }} {{ $slot }} {{ $attributes }}</div>";
        let props: Vec<String> = props_of(card)
            .into_iter()
            .map(|(name, span)| format!("{name}@{}", &card[span.start as usize..span.end as usize]))
            .collect();
        assert_eq!(props, ["title@title", "showCount@showCount"]);
        let slots: Vec<String> = slots_of(card).into_iter().map(|(name, _)| name).collect();
        assert_eq!(slots, ["title", "footer"]);
        assert_eq!(attribute_key("show-view-count"), "showViewCount");
        assert_eq!(attribute_key(":user"), "user");
        assert_eq!(kebab("showViewCount"), "show-view-count");

        let mut directives = Directives::default();
        directives.scan("Blade::directive('error', fn ($e) => 1); Blade::if(\"admin\", fn () => true); $blade->directive('money', $f);");
        assert_eq!(directives.plain, ["error", "money"]);
        assert_eq!(directives.conditionals, ["admin"]);
    }
}
