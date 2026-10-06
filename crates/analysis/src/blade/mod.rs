//! Blade templates as a language of their own. A template is read the way the compiler reads it
//! (`scan`), the names its directives and tags give are kept apart (views, translations, components),
//! and all of its PHP is written out as one document (`compile`): echoes, `@php`, the arguments of
//! the directives, the bound attributes of components, with `@if`, `@foreach` and the rest as the
//! control structures they compile to. That document is what the type layer reads, so a variable
//! `@foreach` names, one `@php` sets or one the template is given has its type wherever it is used,
//! and every question asked of PHP (hover, definition, completion, usages, rename) is asked of it and
//! mapped back.

mod compile;
pub mod data;
pub mod scan;

use std::path::Path;

use php_index::framework::keys::{KeyKind, definitions};
use php_index::framework::overlay::{Marker, directive_markers};
use php_index::{Index, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange, parse};

use crate::ast::range_of;
use crate::completion::{CompletionList, CompletionOptions, complete};
use crate::context::FileContext;
use crate::frameworks::complete::key_items;
use crate::infer::Analyzer;
use crate::nav::{HoverResult, Place, hover_markdown};
use crate::refs::{Access, Hit, HitKind, Query, Symbol, hits_in_file, variable_hits};
use crate::rename::{Prepared, RenameKind};

use compile::Builder;
pub use compile::{Imbalance, Virtual};
use scan::{Directive, Node, scan};

/// A name a directive or a component tag gives: a view, a translation, a component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRef {
    pub kind: KeyKind,
    pub value: String,
    pub start: u32,
    pub end: u32,
}

/// A template, read.
pub struct Template {
    pub nodes: Vec<Node>,
    pub names: Vec<NameRef>,
    pub virt: Virtual,
    pub imbalances: Vec<Imbalance>,
}

/// Whether a file is a Blade template, by its name.
pub fn is_template(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".blade.php"))
}

/// Whether a template is an anonymous component, which gets `$attributes` and `$slot`.
fn is_component(path: Option<&Path>) -> bool {
    path.is_some_and(|path| {
        path.components()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair[0].as_os_str() == "views" && pair[1].as_os_str() == "components")
    })
}

/// The variables every template of a Laravel project has, with the class each one is.
const SHARED: &[(&str, &str)] = &[
    ("errors", "Illuminate\\Support\\ViewErrorBag"),
    ("__env", "Illuminate\\View\\Factory"),
    ("app", "Illuminate\\Contracts\\Foundation\\Application"),
];

const COMPONENT: &[(&str, &str)] = &[
    ("attributes", "Illuminate\\View\\ComponentAttributeBag"),
    ("slot", "Illuminate\\View\\ComponentSlot"),
];

impl Template {
    /// Reads a template, with the variables it is known to be given.
    pub fn read(index: &Index, path: Option<&Path>, text: &str, given: &[(String, Type)]) -> Template {
        let nodes = scan(text);
        let mut names = Vec::new();
        let mut builder = Builder::new(text);
        if index.frameworks().laravel {
            for (name, class) in SHARED {
                declare(&mut builder, name, &Type::class(*class));
            }
            if is_component(path) {
                for (name, class) in COMPONENT {
                    declare(&mut builder, name, &Type::class(*class));
                }
            }
        }
        for (name, ty) in given {
            declare(&mut builder, name, ty);
        }
        for node in &nodes {
            match node {
                Node::Directive(directive) => {
                    names.extend(directive_names(index, text, directive));
                    if directive.is("props") || directive.is("aware") {
                        props(&mut builder, text, directive, given);
                        continue;
                    }
                }
                Node::Tag(tag)
                    if !tag.name.is_empty() && !tag.name.starts_with("slot") && tag.name != "dynamic-component" =>
                {
                    names.push(NameRef {
                        kind: KeyKind::Component,
                        value: tag.name.clone(),
                        start: tag.name_start,
                        end: tag.name_end,
                    });
                }
                _ => {}
            }
            builder.node(node);
        }
        let (virt, imbalances) = builder.finish();
        Template {
            nodes,
            names,
            virt,
            imbalances,
        }
    }

    pub fn root(&self) -> SyntaxNode {
        parse(&self.virt.text).syntax()
    }

    fn name_at(&self, offset: u32) -> Option<&NameRef> {
        self.names
            .iter()
            .find(|name| name.start <= offset && offset <= name.end)
    }
}

/// `/** @var Type $name */ $name;` at the top of the document.
fn declare(builder: &mut Builder<'_>, name: &str, ty: &Type) {
    let written = ty.display_with(&mut |class| format!("\\{class}"));
    builder.head(&format!("/** @var {written} ${name} */\n${name};\n"));
}

/// `@props(['type' => 'info', 'message'])`: each key is a variable, with its default, unless the
/// places that use the component say what it is given.
fn props(builder: &mut Builder<'_>, text: &str, directive: &Directive, given: &[(String, Type)]) {
    let Some((start, end)) = directive.args else {
        return;
    };
    builder.emit("[");
    builder.copy(start, end);
    builder.emit("];\n");
    let wrapped = format!("<?php {};", &text[start as usize..end as usize]);
    let shift = |offset: u32| offset - 6 + start;
    let tree = parse(&wrapped).syntax();
    let Some(array) = tree.descendants().find(|node| node.kind() == ARRAY_EXPR) else {
        return;
    };
    for item in array.children().filter(|node| node.kind() == ARRAY_ITEM) {
        let parts: Vec<SyntaxNode> = item.children().collect();
        let (name, value) = match parts.as_slice() {
            [key, value] => (
                php_index::test_facts::string_value(key).map(|(name, _)| name),
                Some(value),
            ),
            [only] => (php_index::test_facts::string_value(only).map(|(name, _)| name), None),
            _ => continue,
        };
        let Some(name) = name.filter(|name| is_variable_name(name)) else {
            continue;
        };
        if given.iter().any(|(known, _)| *known == name) {
            continue;
        }
        match value {
            Some(value) => {
                builder.emit(&format!("${name} = ("));
                let range = value.text_range();
                builder.copy(shift(u32::from(range.start())), shift(u32::from(range.end())));
                builder.emit(");\n");
            }
            None => builder.emit(&format!("/** @var mixed ${name} */\n${name} = null;\n")),
        }
    }
}

pub(crate) fn is_variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// The names a directive's arguments give, by what the overlay says the directive takes.
fn directive_names(index: &Index, text: &str, directive: &Directive) -> Vec<NameRef> {
    let Some((start, end)) = directive.args else {
        return Vec::new();
    };
    let args = &text[start as usize..end as usize];
    let mut out = Vec::new();
    for marker in directive_markers(index, &directive.name) {
        let Marker::Key { kind, position, .. } = marker else {
            continue;
        };
        let Some(kind) = KeyKind::parse(&kind) else {
            continue;
        };
        if let Some((value, from, to)) = nth_string(args, position) {
            out.push(NameRef {
                kind,
                value,
                start: start + from as u32,
                end: start + to as u32,
            });
        }
    }
    out
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

/// A place of the document as a place of the template; one in another file stays as it is.
fn place_to_source(virt: &Virtual, place: Place) -> Option<Place> {
    if place.path.is_some() {
        return Some(place);
    }
    let range = virt.range_to_source(range_of(place.span.start, place.span.end))?;
    Some(Place {
        path: None,
        span: php_index::Span {
            start: u32::from(range.start()),
            end: u32::from(range.end()),
        },
    })
}

/// Where the name or the PHP under an offset of a template is declared.
pub fn definitions_at(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
) -> Vec<Place> {
    let template = Template::read(index, path, text, given);
    if let Some(name) = template.name_at(offset) {
        return definitions(index, name.kind, &name.value, None)
            .into_iter()
            .map(|definition| Place {
                path: Some(definition.path),
                span: definition.span,
            })
            .collect();
    }
    let Some(at) = template.virt.to_virtual(offset) else {
        return Vec::new();
    };
    let root = template.root();
    Analyzer::new(index, &root, at)
        .definitions(at)
        .into_iter()
        .filter_map(|place| place_to_source(&template.virt, place))
        .collect()
}

/// What the name or the PHP under an offset is.
pub fn hover_at(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
) -> Option<HoverResult> {
    let template = Template::read(index, path, text, given);
    if let Some(name) = template.name_at(offset) {
        let sections: Vec<String> = crate::frameworks::keys::describe(index, name.kind, &name.value, None)
            .iter()
            .map(hover_markdown)
            .collect();
        return Some(HoverResult {
            markdown: sections.join("\n\n---\n\n"),
            range: range_of(name.start, name.end),
        });
    }
    let at = template.virt.to_virtual(offset)?;
    let root = template.root();
    let found = Analyzer::new(index, &root, at).hover(at)?;
    Some(HoverResult {
        markdown: found.markdown,
        range: template.virt.range_to_source(found.range)?,
    })
}

/// What can be typed at an offset of a template.
pub fn complete_at(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
    options: CompletionOptions,
) -> Option<CompletionList> {
    let template = Template::read(index, path, text, given);
    if let Some(name) = template.name_at(offset) {
        let typed = text.get(name.start as usize..offset as usize)?;
        return Some(key_items(
            index,
            name.kind,
            None,
            typed,
            (name.start, name.end),
            options,
        ));
    }
    let at = template.virt.to_virtual(offset)?;
    let mut list = complete(index, &template.virt.text, at, options);
    list.items.retain_mut(|item| {
        let Some(range) = template.virt.range_to_source(range_of(item.edit.start, item.edit.end)) else {
            return false;
        };
        item.edit.start = u32::from(range.start());
        item.edit.end = u32::from(range.end());
        item.additional_edits.clear();
        true
    });
    Some(list)
}

/// What is under an offset of a template, for usages, highlights and rename.
pub fn symbols_at(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
) -> Option<(TextRange, Vec<Symbol>)> {
    let template = Template::read(index, path, text, given);
    if let Some(name) = template.name_at(offset) {
        return Some((
            range_of(name.start, name.end),
            vec![Symbol::Key {
                kind: name.kind,
                name: name.value.clone(),
                scope: None,
            }],
        ));
    }
    let at = template.virt.to_virtual(offset)?;
    let root = template.root();
    let (range, symbols) = crate::references::symbols_at(index, &root, at)?;
    Some((template.virt.range_to_source(range)?, symbols))
}

/// The places of a template that name what a query asks for.
pub fn hits(index: &Index, path: Option<&Path>, text: &str, given: &[(String, Type)], query: &Query) -> Vec<Hit> {
    let template = Template::read(index, path, text, given);
    let mut out = Vec::new();
    if let Symbol::Key { .. } = &query.symbol {
        for name in &template.names {
            let symbol = Symbol::Key {
                kind: name.kind,
                name: name.value.clone(),
                scope: None,
            };
            if query.matches(&symbol) {
                out.push(Hit {
                    range: range_of(name.start, name.end),
                    kind: HitKind::Reference,
                    access: Access::Read,
                    dollar: false,
                    via_alias: false,
                    symbol,
                });
            }
        }
    }
    let root = template.root();
    let ctx = FileContext::new(index, &root);
    let found = match &query.symbol {
        Symbol::Variable { name, scope } => variable_hits(&ctx, *scope, name),
        _ => hits_in_file(&ctx, &template.virt.text, query),
    };
    for mut hit in found {
        if let Some(range) = template.virt.range_to_source(hit.range) {
            hit.range = range;
            out.push(hit);
        }
    }
    out.sort_by_key(|hit| (hit.range.start(), hit.range.end()));
    out.dedup_by_key(|hit| (hit.range.start(), hit.range.end()));
    out
}

/// The name under an offset of a template, when it can be renamed: a variable or anything its PHP
/// names that the project declares.
pub fn prepare_rename(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
) -> Result<Prepared, String> {
    let template = Template::read(index, path, text, given);
    if template.name_at(offset).is_some() {
        return Err("A string that names a route, a key or a view cannot be renamed".to_string());
    }
    let Some(at) = template.virt.to_virtual(offset) else {
        return Err("There is no name to rename here".to_string());
    };
    let root = template.root();
    let prepared = crate::rename::prepare_rename(index, &root, &template.virt.text, at)?;
    let range = template
        .virt
        .range_to_source(prepared.range)
        .ok_or("This name is not written in the template")?;
    Ok(Prepared { range, ..prepared })
}

/// The edits that rename what is under an offset of a template, in it and everywhere else.
pub fn rename(
    index: &Index,
    sources: &dyn crate::references::Sources,
    current: &crate::references::Current,
    offset: u32,
    new_name: &str,
) -> Result<crate::rename::Rename, String> {
    let given = data::given(index, sources, current.path);
    let prepared = prepare_rename(index, Some(current.path), current.text, &given, offset)?;
    if prepared.kind == RenameKind::Namespace {
        return Err("A namespace is renamed from its declaration".to_string());
    }
    let (_, symbols) =
        symbols_at(index, Some(current.path), current.text, &given, offset).ok_or("There is no name to rename here")?;
    crate::rename::rename_symbols(index, sources, current, &symbols, prepared.kind, new_name)
}

#[cfg(test)]
mod tests;
