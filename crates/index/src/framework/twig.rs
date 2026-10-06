//! What Twig reads from PHP and from its templates: the functions, filters and tests the extensions
//! of the project and its packages declare (`new TwigFunction('path', $this->getPath(...))`, Twig's
//! own `CoreExtension` among them), and the parent and the blocks of each template.

use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::Section;
use super::symfony::templates::Templates;
use crate::index::Index;
use crate::model::Span;

const EXTENSION: &str = "Twig\\Extension\\ExtensionInterface";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TwigKind {
    Function,
    Filter,
    Test,
}

/// A function, filter or test, with the PHP that runs for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TwigCallable {
    pub kind: TwigKind,
    pub name: String,
    /// The class of a method, or `None` for a function or a closure.
    pub class: Option<String>,
    /// The method of the class, or the function.
    pub target: Option<String>,
    /// The parameters Twig fills itself before the arguments: the environment, the charset, the
    /// context.
    pub skipped: usize,
    /// Where the name is declared, inside its quotes.
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Default)]
pub struct TwigExtensions {
    pub entries: Vec<TwigCallable>,
}

impl TwigExtensions {
    /// The callable of a name, by its own name or by a pattern such as `render_*`.
    pub fn find(&self, kind: TwigKind, name: &str) -> Option<&TwigCallable> {
        self.entries
            .iter()
            .find(|entry| entry.kind == kind && entry.name == name)
            .or_else(|| {
                self.entries.iter().find(|entry| {
                    entry.kind == kind
                        && entry.name.split_once('*').is_some_and(|(prefix, suffix)| {
                            name.len() > prefix.len() + suffix.len()
                                && name.starts_with(prefix)
                                && name.ends_with(suffix)
                        })
                })
            })
    }

    pub fn of_kind(&self, kind: TwigKind) -> impl Iterator<Item = &TwigCallable> {
        self.entries.iter().filter(move |entry| entry.kind == kind)
    }
}

impl Section for TwigExtensions {
    fn build(index: &Index) -> Self {
        let mut found = TwigExtensions::default();
        let mut paths: Vec<PathBuf> = index
            .all_subtypes(EXTENSION)
            .iter()
            .map(|class| class.file.path.clone())
            .collect();
        paths.sort();
        paths.dedup();
        for path in paths {
            let Some(text) = index.read_text(&path) else {
                continue;
            };
            if !text.contains("Twig") {
                continue;
            }
            read_extension(&path, &text, &mut found.entries);
        }
        for file in index.files() {
            if file.origin != crate::index::Origin::Project {
                continue;
            }
            let marked = index.read_text(&file.path).is_some_and(|text| text.contains("AsTwig"));
            if !marked {
                continue;
            }
            for class in &file.symbols().classes {
                for method in &class.methods {
                    for attribute in &method.attributes {
                        let Some(kind) = attribute_kind(&attribute.name) else {
                            continue;
                        };
                        let named = |key: &str| attribute.args.iter().find(|arg| arg.name.as_deref() == Some(key));
                        let name = named("name")
                            .or_else(|| attribute.args.iter().find(|arg| arg.name.is_none()))
                            .and_then(|arg| unquote(&arg.value));
                        let Some(name) = name else {
                            continue;
                        };
                        let skipped = ["needsEnvironment", "needsCharset", "needsContext", "needsIsSandboxed"]
                            .iter()
                            .filter(|key| named(key).is_some_and(|arg| arg.value.eq_ignore_ascii_case("true")))
                            .count();
                        found.entries.push(TwigCallable {
                            kind,
                            name,
                            class: Some(class.name.clone()),
                            target: Some(method.name.clone()),
                            skipped,
                            path: file.path.clone(),
                            span: method.name_span,
                        });
                    }
                }
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

/// The kind of callable a method attribute declares (Twig 3.21 and later).
fn attribute_kind(name: &str) -> Option<TwigKind> {
    match name.trim_start_matches('\\') {
        "Twig\\Attribute\\AsTwigFunction" => Some(TwigKind::Function),
        "Twig\\Attribute\\AsTwigFilter" => Some(TwigKind::Filter),
        "Twig\\Attribute\\AsTwigTest" => Some(TwigKind::Test),
        _ => None,
    }
}

fn unquote(value: &str) -> Option<String> {
    let value = value.trim();
    let quote = value.chars().next().filter(|quote| matches!(quote, '\'' | '"'))?;
    value.strip_prefix(quote)?.strip_suffix(quote).map(str::to_string)
}

fn kind_of(class: &str) -> Option<TwigKind> {
    match class.trim_start_matches('\\') {
        "Twig\\TwigFunction" | "Twig_SimpleFunction" | "Twig_Function" => Some(TwigKind::Function),
        "Twig\\TwigFilter" | "Twig_SimpleFilter" | "Twig_Filter" => Some(TwigKind::Filter),
        "Twig\\TwigTest" | "Twig_SimpleTest" | "Twig_Test" => Some(TwigKind::Test),
        _ => None,
    }
}

fn read_extension(path: &Path, text: &str, out: &mut Vec<TwigCallable>) {
    let root = parse(text).syntax();
    for new in root.descendants().filter(|node| node.kind() == NEW_EXPR) {
        let Some(name) = new.children().find(|child| child.kind() == NAME) else {
            continue;
        };
        let offset = u32::from(new.text_range().start());
        let resolver = crate::extract::resolver_at(&root, offset);
        let Some(kind) = kind_of(&resolver.resolve_class(&name.text().to_string())) else {
            continue;
        };
        let arguments: Vec<SyntaxNode> = new
            .children()
            .find(|child| child.kind() == ARGUMENT_LIST)
            .map(|list| {
                list.children()
                    .filter(|child| child.kind() == ARGUMENT)
                    .filter_map(|argument| argument.children().last())
                    .collect()
            })
            .unwrap_or_default();
        let Some((twig_name, span)) = arguments.first().and_then(crate::test_facts::string_value) else {
            continue;
        };
        let class_of = |node: &SyntaxNode| {
            node.ancestors()
                .find(|ancestor| matches!(ancestor.kind(), CLASS_DECLARATION | TRAIT_DECLARATION))
                .and_then(|class| class.children().find(|child| child.kind() == NAME))
                .map(|class| resolver.resolve_class(&class.text().to_string()))
        };
        let (class, target) = arguments
            .get(1)
            .map(|callable| callable_of(callable, &resolver, class_of(&new)))
            .unwrap_or((None, None));
        let skipped = arguments.get(2).map_or(0, needs);
        out.push(TwigCallable {
            kind,
            name: twig_name,
            class,
            target,
            skipped,
            path: path.to_path_buf(),
            span,
        });
    }
}

/// The method or function a callable names: `[$this, 'm']`, `[X::class, 'm']`, `'f'`,
/// `$this->m(...)` or `X::m(...)`.
fn callable_of(
    node: &SyntaxNode,
    resolver: &crate::resolve::NameResolver,
    own: Option<String>,
) -> (Option<String>, Option<String>) {
    match node.kind() {
        LITERAL => match crate::test_facts::string_value(node) {
            Some((text, _)) => match text.split_once("::") {
                Some((class, method)) => (Some(resolver.resolve_class(class)), Some(method.to_string())),
                None => (None, Some(text)),
            },
            None => (None, None),
        },
        ARRAY_EXPR => {
            let items: Vec<SyntaxNode> = node
                .children()
                .filter(|child| child.kind() == ARRAY_ITEM)
                .filter_map(|item| item.children().last())
                .collect();
            let [subject, method] = items.as_slice() else {
                return (None, None);
            };
            let Some((method, _)) = crate::test_facts::string_value(method) else {
                return (None, None);
            };
            let class = match subject.kind() {
                VARIABLE_EXPR if subject.text() == "$this" => own,
                SCOPED_ACCESS_EXPR => class_constant_class(subject, resolver, own),
                _ => crate::test_facts::string_value(subject).map(|(class, _)| resolver.resolve_class(&class)),
            };
            (class, Some(method))
        }
        CALL_EXPR => {
            let Some(callee) = node.children().next() else {
                return (None, None);
            };
            let method = callee
                .children()
                .filter(|child| child.kind() == NAME)
                .last()
                .map(|name| name.text().to_string());
            let class = match callee.kind() {
                PROPERTY_FETCH_EXPR => own,
                SCOPED_ACCESS_EXPR => callee
                    .children()
                    .find(|child| child.kind() == NAME)
                    .map(|name| scoped_class(&name.text().to_string(), resolver, own)),
                NAME => return (None, Some(callee.text().to_string())),
                _ => None,
            };
            (class, method)
        }
        _ => (None, None),
    }
}

fn scoped_class(written: &str, resolver: &crate::resolve::NameResolver, own: Option<String>) -> String {
    match written.to_ascii_lowercase().as_str() {
        "self" | "static" => own.unwrap_or_default(),
        _ => resolver.resolve_class(written),
    }
}

/// The class of `X::class`.
fn class_constant_class(
    node: &SyntaxNode,
    resolver: &crate::resolve::NameResolver,
    own: Option<String>,
) -> Option<String> {
    let text = node.text().to_string();
    let class = text.strip_suffix("::class")?;
    Some(scoped_class(class.trim(), resolver, own))
}

/// How many parameters Twig fills before the arguments, from the options of the declaration.
fn needs(options: &SyntaxNode) -> usize {
    if options.kind() != ARRAY_EXPR {
        return 0;
    }
    options
        .children()
        .filter(|child| child.kind() == ARRAY_ITEM)
        .filter(|item| {
            let parts: Vec<SyntaxNode> = item.children().collect();
            let [key, value] = parts.as_slice() else {
                return false;
            };
            let named = crate::test_facts::string_value(key).is_some_and(|(key, _)| {
                matches!(key.as_str(), "needs_environment" | "needs_charset" | "needs_context")
            });
            named && value.text().to_string().eq_ignore_ascii_case("true")
        })
        .count()
}

/// The parent and the blocks of a template, read from its tags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TemplateShape {
    pub extends: Option<String>,
    pub blocks: Vec<(String, Span)>,
}

#[derive(Default)]
pub struct TwigShapes {
    shapes: Vec<(PathBuf, TemplateShape)>,
}

impl TwigShapes {
    pub fn of(&self, path: &Path) -> Option<&TemplateShape> {
        self.shapes
            .iter()
            .find(|(known, _)| known == path)
            .map(|(_, shape)| shape)
    }

    pub fn all(&self) -> impl Iterator<Item = &(PathBuf, TemplateShape)> {
        self.shapes.iter()
    }
}

impl Section for TwigShapes {
    fn build(index: &Index) -> Self {
        let templates = index.section::<Templates>();
        let shapes = templates
            .templates
            .iter()
            .filter_map(|template| {
                let text = index.read_text(&template.path)?;
                Some((template.path.clone(), shape_of(&text)))
            })
            .collect();
        TwigShapes { shapes }
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_below(root, path, "templates")
    }
}

/// The parent and the blocks of a template's text.
pub fn shape_of(text: &str) -> TemplateShape {
    let mut shape = TemplateShape::default();
    let mut at = 0;
    while let Some(found) = text[at..].find("{%") {
        let start = at + found + 2;
        at = start;
        let rest = text[start..].trim_start_matches(['-', '~']);
        let skipped = text[start..].len() - rest.len();
        let inner = rest.trim_start();
        let word_start = start + skipped + (rest.len() - inner.len());
        let word: String = inner
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        let after = word_start + word.len();
        let tail = &text[after..];
        let name_offset = after + (tail.len() - tail.trim_start().len());
        let tail = tail.trim_start();
        match word.as_str() {
            "extends" if shape.extends.is_none() => {
                if let Some(quote) = tail.chars().next().filter(|quote| matches!(quote, '\'' | '"')) {
                    if let Some(end) = tail[1..].find(quote) {
                        shape.extends = Some(tail[1..1 + end].to_string());
                    }
                }
            }
            "block" => {
                let name: String = tail
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    shape.blocks.push((
                        name.clone(),
                        Span {
                            start: name_offset as u32,
                            end: (name_offset + name.len()) as u32,
                        },
                    ));
                }
            }
            _ => {}
        }
    }
    shape
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_functions_filters_and_tests_of_an_extension() {
        let text = "<?php\nnamespace App\\Twig;\nuse Twig\\Extension\\AbstractExtension;\nuse Twig\\{TwigFilter, TwigFunction, TwigTest};\nclass AppExtension extends AbstractExtension {\n    public function getFilters(): array { return [new TwigFilter('md', [$this, 'markdown'], ['is_safe' => ['html'], 'needs_environment' => true]), new TwigFilter('abs', 'abs')]; }\n    public function getFunctions(): array { return [new TwigFunction('path', $this->getPath(...)), new TwigFunction('helper', [Helper::class, 'run'])]; }\n    public function getTests(): array { return [new TwigTest('odd', fn ($x) => $x % 2)]; }\n}\n";
        let mut found = Vec::new();
        read_extension(Path::new("/project/src/Twig/AppExtension.php"), text, &mut found);
        let shown: Vec<String> = found
            .iter()
            .map(|entry| {
                format!(
                    "{:?} {} {}::{} {}",
                    entry.kind,
                    entry.name,
                    entry.class.as_deref().unwrap_or("-"),
                    entry.target.as_deref().unwrap_or("-"),
                    entry.skipped
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                "Filter md App\\Twig\\AppExtension::markdown 1",
                "Filter abs -::abs 0",
                "Function path App\\Twig\\AppExtension::getPath 0",
                "Function helper App\\Twig\\Helper::run 0",
                "Test odd -::- 0",
            ]
        );
    }

    #[test]
    fn reads_attributes_and_patterns() {
        let index = crate::framework::testing::project(&[(
            "src/Twig/AppExtension.php",
            "<?php\nnamespace App\\Twig;\nuse Twig\\Attribute\\{AsTwigFilter, AsTwigFunction};\nfinal class AppExtension {\n    #[AsTwigFunction('locales')]\n    public function getLocales(): array {}\n    #[AsTwigFilter(name: 'md', needsEnvironment: true)]\n    public function markdown($env, string $text): string {}\n}\n",
        )]);
        let extensions = index.section::<TwigExtensions>();
        let locales = extensions.find(TwigKind::Function, "locales").expect("a function");
        assert_eq!(locales.target.as_deref(), Some("getLocales"));
        let markdown = extensions.find(TwigKind::Filter, "md").expect("a filter");
        assert_eq!(markdown.skipped, 1);
        let mut patterns = TwigExtensions::default();
        patterns.entries.push(TwigCallable {
            kind: TwigKind::Function,
            name: "render_*".to_string(),
            class: None,
            target: None,
            skipped: 0,
            path: PathBuf::new(),
            span: Span::default(),
        });
        assert!(patterns.find(TwigKind::Function, "render_esi").is_some());
        assert!(patterns.find(TwigKind::Function, "render_").is_none());
    }

    #[test]
    fn reads_the_parent_and_the_blocks_of_a_template() {
        let text = "{% extends 'base.html.twig' %}\n{% block title 'x' %}{%- block body -%}{% block inner %}{% endblock %}{% endblock %}";
        let shape = shape_of(text);
        assert_eq!(shape.extends.as_deref(), Some("base.html.twig"));
        let names: Vec<&str> = shape.blocks.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["title", "body", "inner"]);
        let (_, span) = &shape.blocks[1];
        assert_eq!(&text[span.start as usize..span.end as usize], "body");
    }
}
