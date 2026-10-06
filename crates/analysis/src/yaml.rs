//! The YAML of a Symfony project's `config/` and `translations/`, read for the names in it: a
//! `%parameter%` and `%env(NAME)%` in any string, `@service` references, the classes services are
//! declared by (`App\Mailer:` and `class: App\Mailer`), and `controller: App\Controller\Blog::show`
//! in route files. They complete, hover, lead to their declarations and are usages of them, and a
//! class that does not exist is reported.

use std::path::Path;

use php_index::framework::keys::{KeyKind, declared_at, definitions};
use php_index::framework::yaml::{Node, parse};
use php_index::{Index, Span};
use php_syntax::TextRange;

use crate::ast::range_of;
use crate::completion::{CompletionItem, CompletionList, CompletionOptions, ItemKind, TextEdit, match_score};
use crate::diagnostics::Diagnostic;
use crate::frameworks::complete::key_items;
use crate::inspections::{INSPECTIONS, InspectionSettings};
use crate::nav::{HoverResult, Place, hover_markdown};
use crate::refs::{Access, Hit, HitKind, Query, Symbol};
use crate::target::Target;

/// Whether a file is YAML of a Symfony project's configuration or translations.
pub fn is_config(index: &Index, path: &Path) -> bool {
    let yaml = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "yaml" | "yml"));
    let root = index.framework_root();
    yaml && index.frameworks().symfony
        && ["config", "translations"].iter().any(|folder| {
            path.strip_prefix(root)
                .is_ok_and(|relative| relative.starts_with(folder))
        })
}

/// A name a scalar of the document gives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ref {
    Key {
        kind: KeyKind,
        name: String,
        start: u32,
        end: u32,
    },
    /// A class, with where its last segment is, which is what a rename changes.
    Class {
        name: String,
        start: u32,
        end: u32,
        last: u32,
    },
    Method {
        class: String,
        name: String,
        start: u32,
        end: u32,
    },
}

impl Ref {
    fn range(&self) -> (u32, u32) {
        match self {
            Ref::Key { start, end, .. } | Ref::Class { start, end, .. } | Ref::Method { start, end, .. } => {
                (*start, *end)
            }
        }
    }
}

/// The names of a document, read from its tree.
pub fn refs(text: &str, path: &Path, root_dir: &Path) -> Vec<Ref> {
    let Some(root) = parse(text) else {
        return Vec::new();
    };
    let relative = path.strip_prefix(root_dir).unwrap_or(path);
    let routes = relative.starts_with("config/routes")
        || relative
            .file_stem()
            .is_some_and(|stem| stem.to_string_lossy().starts_with("routes"));
    let mut out = Vec::new();
    walk(text, &root, &mut out);
    if let Some(services) = root.get("services") {
        for (key, span, value) in services.entries() {
            if is_class_name(key) && written(text, span, key) {
                out.push(class_ref(key, span));
            }
            if let Some(Node::Scalar { value: class, span }) = value.get("class") {
                if is_class_name(class) && written(text, *span, class) {
                    out.push(class_ref(class, *span));
                }
            }
        }
    }
    if routes {
        for (_, _, route) in root.entries() {
            if let Some(Node::Scalar { value, span }) = route.get("controller") {
                if written(text, *span, value) {
                    out.extend(controller_refs(value, *span));
                }
            }
        }
    }
    out.sort_by_key(|reference| reference.range());
    out.dedup();
    out
}

/// Whether the text at a span is the scalar as read, which a quoted string with escapes is not.
fn written(text: &str, span: Span, value: &str) -> bool {
    text.get(span.start as usize..span.end as usize) == Some(value)
}

fn is_class_name(name: &str) -> bool {
    name.contains('\\')
        && !name.ends_with('\\')
        && !name.contains(['%', '@', ' ', ':', '/'])
        && name
            .trim_start_matches('\\')
            .split('\\')
            .all(|segment| !segment.is_empty() && segment.chars().all(|c| c.is_alphanumeric() || c == '_'))
}

fn class_ref(name: &str, span: Span) -> Ref {
    let last = name.rfind('\\').map_or(0, |at| at + 1) as u32;
    Ref::Class {
        name: name.trim_start_matches('\\').to_string(),
        start: span.start,
        end: span.end,
        last: span.start + last,
    }
}

/// `App\Controller\Blog::show`.
fn controller_refs(value: &str, span: Span) -> Vec<Ref> {
    let Some((class, method)) = value.split_once("::") else {
        return if is_class_name(value) {
            vec![class_ref(value, span)]
        } else {
            Vec::new()
        };
    };
    if !is_class_name(class) {
        return Vec::new();
    }
    let class_span = Span {
        start: span.start,
        end: span.start + class.len() as u32,
    };
    let method_start = class_span.end + 2;
    vec![
        class_ref(class, class_span),
        Ref::Method {
            class: class.trim_start_matches('\\').to_string(),
            name: method.to_string(),
            start: method_start,
            end: method_start + method.len() as u32,
        },
    ]
}

/// The placeholders and service references of every scalar.
fn walk(text: &str, node: &Node, out: &mut Vec<Ref>) {
    match node {
        Node::Scalar { value, span } => {
            if !written(text, *span, value) {
                return;
            }
            placeholders(value, span.start, out);
            if let Some(id) = value.strip_prefix('@').filter(|id| !id.starts_with(['=', '@'])) {
                let id = id.strip_prefix('?').unwrap_or(id);
                if !id.is_empty() && !id.contains(' ') {
                    let start = span.end - id.len() as u32;
                    out.push(Ref::Key {
                        kind: KeyKind::Service,
                        name: id.to_string(),
                        start,
                        end: span.end,
                    });
                }
            }
        }
        Node::Map(entries) => {
            for (key, value) in entries {
                walk(text, key, out);
                walk(text, value, out);
            }
        }
        Node::Seq(items) => items.iter().for_each(|item| walk(text, item, out)),
    }
}

/// `%name%` and `%env(PROCESSOR:NAME)%` inside a string.
fn placeholders(value: &str, start: u32, out: &mut Vec<Ref>) {
    let mut from = 0;
    while let Some(open) = value[from..].find('%') {
        let begin = from + open;
        if value[begin + 1..].starts_with('%') {
            from = begin + 2;
            continue;
        }
        let Some(length) = value[begin + 1..].find('%') else {
            break;
        };
        let inner_start = begin + 1;
        let inner = &value[inner_start..inner_start + length];
        from = inner_start + length + 1;
        if inner.is_empty() || inner.contains(' ') {
            continue;
        }
        let (kind, offset, name) = match inner.strip_prefix("env(").and_then(|rest| rest.strip_suffix(')')) {
            Some(env) => {
                let skipped = env.rfind(':').map_or(0, |colon| colon + 1);
                (KeyKind::Env, inner_start + 4 + skipped, &env[skipped..])
            }
            None => (KeyKind::Parameter, inner_start, inner),
        };
        out.push(Ref::Key {
            kind,
            name: name.to_string(),
            start: start + offset as u32,
            end: start + (offset + name.len()) as u32,
        });
    }
}

fn ref_at(refs: &[Ref], offset: u32) -> Option<&Ref> {
    refs.iter().find(|reference| {
        let (start, end) = reference.range();
        start <= offset && offset <= end
    })
}

fn symbol_of(reference: &Ref) -> Symbol {
    match reference {
        Ref::Key { kind, name, .. } => Symbol::Key {
            kind: *kind,
            name: name.clone(),
            scope: None,
        },
        Ref::Class { name, .. } => Symbol::Class(name.clone()),
        Ref::Method { class, name, .. } => Symbol::Method {
            class: class.clone(),
            name: name.clone(),
        },
    }
}

/// What is under an offset of a document, for usages: a name it refers to, or one it declares (a
/// parameter, a service, a route, a translation key).
pub fn symbols_at(index: &Index, path: &Path, text: &str, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    let refs = refs(text, path, index.framework_root());
    if let Some(reference) = ref_at(&refs, offset) {
        let (start, end) = match reference {
            Ref::Class { last, end, .. } => (*last, *end),
            other => other.range(),
        };
        let symbol = match reference {
            Ref::Method { class, name, .. } => {
                let found = index.find_method(&php_index::Type::class(class.clone()), name)?;
                Symbol::Method {
                    class: found.class.decl.name.clone(),
                    name: found.member.name.clone(),
                }
            }
            other => symbol_of(other),
        };
        return Some((range_of(start, end), vec![symbol]));
    }
    let (kind, name, span) = declared_at(index, path, offset)?;
    Some((
        range_of(span.start, span.end),
        vec![Symbol::Key {
            kind,
            name,
            scope: None,
        }],
    ))
}

/// The places of a document that name what a query asks for.
pub fn hits(index: &Index, path: &Path, text: &str, query: &Query) -> Vec<Hit> {
    let mut out = Vec::new();
    for reference in refs(text, path, index.framework_root()) {
        let symbol = match &reference {
            Ref::Method { class, name, .. } => {
                let Some(found) = index.find_method(&php_index::Type::class(class.clone()), name) else {
                    continue;
                };
                Symbol::Method {
                    class: found.class.decl.name.clone(),
                    name: found.member.name.clone(),
                }
            }
            other => symbol_of(other),
        };
        if !query.matches(&symbol) {
            continue;
        }
        let (start, end) = match &reference {
            Ref::Class { last, end, .. } => (*last, *end),
            other => other.range(),
        };
        out.push(Hit {
            range: range_of(start, end),
            kind: HitKind::Reference,
            access: Access::Read,
            dollar: false,
            via_alias: false,
            symbol,
        });
    }
    out
}

/// The places a name of the document is declared.
fn places(index: &Index, reference: &Ref) -> Vec<Place> {
    match reference {
        Ref::Key { kind, name, .. } => definitions(index, *kind, name, None)
            .into_iter()
            .map(|definition| Place {
                path: Some(definition.path),
                span: definition.span,
            })
            .collect(),
        _ => describe(index, reference)
            .into_iter()
            .filter_map(|description| description.place)
            .collect(),
    }
}

fn describe(index: &Index, reference: &Ref) -> Vec<crate::nav::Description> {
    let root = php_syntax::parse("<?php ").syntax();
    let analyzer = crate::infer::Analyzer::new(index, &root, 0);
    match reference {
        Ref::Key { kind, name, .. } => crate::frameworks::keys::describe(index, *kind, name, None),
        Ref::Class { name, .. } => analyzer.describe(&Target::Class(name.clone())),
        Ref::Method { class, name, .. } => analyzer.describe(&Target::Method {
            receiver: php_index::Type::class(class.clone()),
            name: name.clone(),
        }),
    }
}

pub fn definitions_at(index: &Index, path: &Path, text: &str, offset: u32) -> Vec<Place> {
    let refs = refs(text, path, index.framework_root());
    ref_at(&refs, offset)
        .map(|reference| places(index, reference))
        .unwrap_or_default()
}

pub fn hover_at(index: &Index, path: &Path, text: &str, offset: u32) -> Option<HoverResult> {
    let refs = refs(text, path, index.framework_root());
    let reference = ref_at(&refs, offset)?;
    let sections: Vec<String> = describe(index, reference).iter().map(hover_markdown).collect();
    let (start, end) = reference.range();
    (!sections.is_empty()).then(|| HoverResult {
        markdown: sections.join("\n\n---\n\n"),
        range: range_of(start, end),
    })
}

/// The word being typed at an offset: what follows `%`, `@` or the start of a scalar, read from the
/// text so a document that does not parse yet still completes.
fn typed_at(text: &str, offset: u32) -> Option<(usize, &str, Option<char>)> {
    let before = text.get(..offset as usize)?;
    let start = before
        .rfind(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '[' | ',' | '{' | '%' | '@' | '(' | ':'))
        .map_or(0, |at| at + 1);
    let lead = before[..start].chars().last();
    Some((start, &before[start..], lead))
}

pub fn complete_at(index: &Index, text: &str, offset: u32, options: CompletionOptions) -> Option<CompletionList> {
    let (start, typed, lead) = typed_at(text, offset)?;
    let end = offset
        + text[offset as usize..]
            .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ']' | ',' | '}' | '%'))
            .unwrap_or(text.len() - offset as usize) as u32;
    let range = (start as u32, end);
    match lead {
        Some('%') if !typed.starts_with("env(") => {
            Some(key_items(index, KeyKind::Parameter, None, typed, range, options))
        }
        Some('(') if text[..start].ends_with("%env(") => {
            Some(key_items(index, KeyKind::Env, None, typed, range, options))
        }
        Some('@') => {
            let mut list = key_items(index, KeyKind::Service, None, typed, range, options);
            list.items.extend(class_items(index, typed, range, options).items);
            Some(list)
        }
        _ if typed.contains('\\') || is_class_position(text, start) => Some(class_items(index, typed, range, options)),
        _ => None,
    }
}

/// Whether a scalar starts where a class is written: after `class:` or `controller:`.
fn is_class_position(text: &str, start: usize) -> bool {
    let line_start = text[..start].rfind('\n').map_or(0, |at| at + 1);
    let line = text[line_start..start].trim();
    line.ends_with("class:") || line.ends_with("controller:")
}

/// The classes of the project and its packages whose name fits what was typed, written in full.
fn class_items(index: &Index, typed: &str, (start, end): (u32, u32), options: CompletionOptions) -> CompletionList {
    let typed = typed.trim_start_matches('\\');
    let mut items: Vec<(u8, CompletionItem)> = index
        .class_names()
        .filter(|class| class.file.origin != php_index::Origin::Stub)
        .filter_map(|class| {
            let name = class.summary.name.clone();
            let score = if typed.contains('\\') {
                name.to_ascii_lowercase()
                    .starts_with(&typed.to_ascii_lowercase())
                    .then_some(0)?
            } else {
                match_score(crate::short(&name), typed)?
            };
            Some((
                score,
                CompletionItem {
                    label: name.clone(),
                    kind: ItemKind::Class,
                    detail: None,
                    description: None,
                    edit: TextEdit {
                        start,
                        end,
                        new_text: name.clone(),
                    },
                    additional_edits: Vec::new(),
                    sort_text: name.clone(),
                    filter_text: Some(name),
                    deprecated: false,
                    data: None,
                },
            ))
        })
        .collect();
    items.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.label.cmp(&right.1.label)));
    let incomplete = items.len() > options.limit;
    CompletionList {
        items: items.into_iter().take(options.limit).map(|(_, item)| item).collect(),
        incomplete,
    }
}

/// A class a service is declared by that does not exist. `loadable` says whether Composer could
/// load a class the index has not read.
pub fn diagnostics(
    index: &Index,
    path: &Path,
    text: &str,
    settings: &InspectionSettings,
    ready: bool,
    loadable: &dyn Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    let Some(severity) = INSPECTIONS
        .iter()
        .find(|info| info.code == "undefined-class")
        .and_then(|info| settings.severity_of(info))
    else {
        return Vec::new();
    };
    if !ready {
        return Vec::new();
    }
    refs(text, path, index.framework_root())
        .into_iter()
        .filter_map(|reference| match reference {
            Ref::Class { name, start, end, .. } if index.class(&name).is_none() && !loadable(&name) => {
                Some(Diagnostic {
                    range: range_of(start, end),
                    message: format!("Undefined class '{name}'"),
                    severity,
                    deprecated: false,
                    unnecessary: false,
                    code: "undefined-class",
                })
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use php_index::framework::testing::SYMFONY;

    use super::*;
    use crate::testing::{CURSOR, Fixture};

    const SERVICES: &str = "parameters:\n    app.locale: 'en'\n\nservices:\n    App\\:\n        resource: '../src/'\n    app.mailer:\n        class: App\\Service\\Mailer\n        arguments: ['%app.locale%', '@logger', '%env(int:MAX)%']\n    App\\Service\\Missing: ~\n";

    fn fixture() -> Fixture {
        let mut files = SYMFONY.to_vec();
        files.extend_from_slice(&[
            ("config/services.yaml", SERVICES),
            (
                "src/Service/Mailer.php",
                "<?php namespace App\\Service; class Mailer { public function send(): bool {} }",
            ),
            (
                "src/Controller/BlogController.php",
                "<?php namespace App\\Controller; class BlogController { public function show() {} }",
            ),
            (
                "config/routes.yaml",
                "blog_show:\n    path: /blog\n    controller: App\\Controller\\BlogController::show\n",
            ),
            (".env", "MAX=3\n"),
        ]);
        Fixture::framework(&files)
    }

    fn shown(text: &str, path: &str) -> Vec<String> {
        refs(text, Path::new(path), Path::new("/project"))
            .into_iter()
            .map(|reference| match reference {
                Ref::Key { kind, name, .. } => format!("{} {name}", kind.label()),
                Ref::Class { name, .. } => format!("class {name}"),
                Ref::Method { class, name, .. } => format!("method {class}::{name}"),
            })
            .collect()
    }

    #[test]
    fn reads_the_names_of_a_services_and_a_routes_file() {
        assert_eq!(
            shown(SERVICES, "/project/config/services.yaml"),
            [
                "class App\\Service\\Mailer",
                "parameter app.locale",
                "service logger",
                "environment variable MAX",
                "class App\\Service\\Missing"
            ]
        );
        assert_eq!(
            shown(
                "blog_show:\n    controller: App\\Controller\\BlogController::show\n",
                "/project/config/routes.yaml"
            ),
            [
                "class App\\Controller\\BlogController",
                "method App\\Controller\\BlogController::show"
            ]
        );
    }

    #[test]
    fn names_lead_to_their_declarations_and_a_missing_class_is_reported() {
        let fixture = fixture();
        let path = Path::new("/project/config/services.yaml");
        let at = |needle: &str| SERVICES.find(needle).expect("the text") as u32 + 2;
        let place = |offset: u32| -> Vec<String> {
            definitions_at(&fixture.index, path, SERVICES, offset)
                .into_iter()
                .map(|place| place.path.map(|path| path.display().to_string()).unwrap_or_default())
                .collect()
        };
        assert_eq!(place(at("Mailer")), ["/project/src/Service/Mailer.php"]);
        assert_eq!(place(at("%app.locale%")), ["/project/config/services.yaml"]);
        assert_eq!(place(at("MAX")), ["/project/.env"]);
        let found = diagnostics(
            &fixture.index,
            path,
            SERVICES,
            &InspectionSettings::default(),
            true,
            &|_| false,
        );
        let codes: Vec<&str> = found
            .iter()
            .map(|found| &SERVICES[usize::from(found.range.start())..usize::from(found.range.end())])
            .collect();
        assert_eq!(codes, ["App\\Service\\Missing"]);
    }

    #[test]
    fn completes_parameters_services_and_classes() {
        let fixture = fixture();
        let complete = |text: &str| -> Vec<String> {
            let offset = text.find(CURSOR).expect("a cursor") as u32;
            let text = text.replacen(CURSOR, "", 1);
            complete_at(&fixture.index, &text, offset, CompletionOptions::default())
                .map(|list| list.items.into_iter().map(|item| item.label).collect())
                .unwrap_or_default()
        };
        assert_eq!(complete("x: '%app.lo$0'"), ["app.locale"]);
        assert!(complete("x: '@app.ma$0'").contains(&"app.mailer".to_string()));
        assert_eq!(complete("    class: App\\Service\\Ma$0"), ["App\\Service\\Mailer"]);
    }

    #[test]
    fn usages_of_a_class_and_a_method_reach_the_configuration() {
        use crate::references::{Current, references_at};
        use crate::testing::Files;
        let fixture = fixture();
        let source = fixture.sources[std::path::Path::new("/project/src/Controller/BlogController.php")].clone();
        let path = std::path::PathBuf::from("/project/src/Controller/BlogController.php");
        let root = php_syntax::parse(&source).syntax();
        let current = Current {
            path: &path,
            text: &source,
            root: &root,
        };
        let files = |needle: &str| -> Vec<String> {
            let offset = source.find(needle).expect("the name") as u32 + 1;
            references_at(&fixture.index, &Files(fixture.sources.clone()), &current, offset)
                .expect("usages")
                .files
                .iter()
                .filter(|file| file.hits.iter().any(|hit| hit.kind != HitKind::Declaration))
                .map(|file| file.path.display().to_string())
                .collect()
        };
        assert_eq!(files("BlogController"), ["/project/config/routes.yaml"]);
        assert_eq!(files("show"), ["/project/config/routes.yaml"]);
    }
}
