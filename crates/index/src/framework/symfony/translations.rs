//! Symfony's translations: `translations/<domain>.<locale>.<format>`, read for YAML, XLIFF and PHP.

use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use crate::framework::Section;
use crate::framework::source::{Literal, array_items, literal_of};
use crate::framework::yaml::{Node, parse as parse_yaml};
use crate::index::Index;
use crate::model::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct SfTranslation {
    pub key: String,
    pub domain: String,
    pub locale: String,
    pub value: Option<String>,
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Default)]
pub struct SfTranslations {
    pub entries: Vec<SfTranslation>,
}

impl SfTranslations {
    /// Where a key is translated, in the given domain or in any.
    pub fn find(&self, key: &str, domain: Option<&str>) -> Vec<&SfTranslation> {
        self.entries
            .iter()
            .filter(|entry| entry.key == key && domain.is_none_or(|domain| entry.domain == domain))
            .collect()
    }
}

/// `messages.en.yaml` is the domain `messages` in the locale `en`.
fn names_of(path: &Path) -> Option<(String, String)> {
    let stem = path.file_stem()?.to_string_lossy().into_owned();
    let (domain, locale) = stem.rsplit_once('.')?;
    let domain = domain.split('+').next().unwrap_or(domain);
    Some((domain.to_string(), locale.to_string()))
}

impl Section for SfTranslations {
    fn build(index: &Index) -> Self {
        let mut found = SfTranslations::default();
        let dir = index.framework_root().join("translations");
        for path in index.files_below(&dir) {
            let Some((domain, locale)) = names_of(&path) else {
                continue;
            };
            let Some(text) = index.read_text(&path) else {
                continue;
            };
            let extension = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
            let mut push = |key: String, value: Option<String>, span: Span| {
                found.entries.push(SfTranslation {
                    key,
                    domain: domain.clone(),
                    locale: locale.clone(),
                    value,
                    path: path.clone(),
                    span,
                });
            };
            match extension {
                "yaml" | "yml" => {
                    if let Some(root) = parse_yaml(&text) {
                        flatten_yaml(&root, "", &mut push);
                    }
                }
                "xlf" | "xliff" => read_xliff(&text, &mut push),
                "php" => {
                    let tree = parse(&text).syntax();
                    if let Some(array) = tree
                        .children()
                        .filter(|node| node.kind() == RETURN_STATEMENT)
                        .filter_map(|node| node.children().next())
                        .find(|node| node.kind() == ARRAY_EXPR)
                    {
                        flatten_php(&array, "", &mut push);
                    }
                }
                _ => {}
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_below(root, path, "translations")
    }
}

fn flatten_yaml(node: &Node, prefix: &str, push: &mut dyn FnMut(String, Option<String>, Span)) {
    for (name, span, value) in node.entries() {
        let key = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        };
        match value {
            Node::Map(_) => flatten_yaml(value, &key, push),
            other => push(key, other.as_str().map(str::to_string), span),
        }
    }
}

fn flatten_php(array: &SyntaxNode, prefix: &str, push: &mut dyn FnMut(String, Option<String>, Span)) {
    for (key, value) in array_items(array).unwrap_or_default() {
        let Some(Literal::Text(name, span)) = key.as_ref().and_then(literal_of) else {
            continue;
        };
        let full = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}.{name}")
        };
        if value.kind() == ARRAY_EXPR {
            flatten_php(&value, &full, push);
        } else {
            let text = match literal_of(&value) {
                Some(Literal::Text(text, _)) => Some(text),
                _ => None,
            };
            push(full, text, span);
        }
    }
}

/// XLIFF 1.2 `<trans-unit id>` with a `<source>`, and 2.0 `<unit id>`: the key is the source of a unit
/// that has one, else its id.
fn read_xliff(text: &str, push: &mut dyn FnMut(String, Option<String>, Span)) {
    let mut rest = 0;
    while let Some(at) = text[rest..].find("<source>") {
        let start = rest + at + "<source>".len();
        let Some(length) = text[start..].find("</source>") else {
            break;
        };
        let key = text[start..start + length].trim().to_string();
        let value = text[start + length..].find("<target").and_then(|target| {
            let from = start + length + target;
            let begin = text[from..].find('>')? + from + 1;
            let end = text[begin..].find("</target>")? + begin;
            (end - begin < 400).then(|| text[begin..end].to_string())
        });
        push(
            key.clone(),
            value,
            Span {
                start: start as u32,
                end: (start + length) as u32,
            },
        );
        rest = start + length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn reads_yaml_php_and_xliff() {
        let index = project(&[
            (
                "translations/messages.en.yaml",
                "app:\n    title: Hello\n    nested:\n        deep: Deep\nplain: Plain\n",
            ),
            ("translations/messages.nl.yaml", "app:\n    title: Hallo\n"),
            (
                "translations/validators.en.php",
                "<?php return ['too_short' => 'Too short', 'group' => ['a' => 'A']];",
            ),
            (
                "translations/security.en.xlf",
                "<?xml version=\"1.0\"?><xliff><file><body><trans-unit id=\"x\"><source>Invalid credentials.</source><target>Wrong</target></trans-unit></body></file></xliff>",
            ),
        ]);
        let translations = index.section::<SfTranslations>();
        assert_eq!(translations.find("app.title", Some("messages")).len(), 2);
        assert_eq!(translations.find("app.nested.deep", None).len(), 1);
        assert_eq!(translations.find("plain", None)[0].value.as_deref(), Some("Plain"));
        assert_eq!(translations.find("group.a", Some("validators")).len(), 1);
        let credentials = translations.find("Invalid credentials.", Some("security"));
        assert_eq!(credentials[0].value.as_deref(), Some("Wrong"));
        assert_eq!(credentials[0].locale, "en");
    }
}
