//! The dotted keys of the arrays that config and translation files return.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::source::{Literal, array_items, literal_of};
use crate::model::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct KeyEntry {
    /// With dots: `app.name`.
    pub key: String,
    /// The value is an array, so the key names a group.
    pub group: bool,
    /// The value as written, when it is short.
    pub value: Option<String>,
    pub path: PathBuf,
    /// The key inside its quotes, or nothing for the file itself.
    pub span: Span,
    /// The locale of a translation.
    pub locale: Option<String>,
}

#[derive(Default)]
pub struct KeyTree {
    pub entries: Vec<KeyEntry>,
    by_key: HashMap<String, Vec<usize>>,
    /// Keys whose children are not known: a value that is computed, or a list.
    open: HashSet<String>,
    roots: HashSet<String>,
}

impl KeyTree {
    pub fn find(&self, key: &str) -> Option<&KeyEntry> {
        self.by_key
            .get(key)
            .and_then(|found| found.first())
            .map(|at| &self.entries[*at])
    }

    /// Every place a key is written, one per locale for a translation.
    pub fn find_all(&self, key: &str) -> Vec<&KeyEntry> {
        self.by_key
            .get(key)
            .map(|found| found.iter().map(|at| &self.entries[*at]).collect())
            .unwrap_or_default()
    }

    pub fn has_root(&self, root: &str) -> bool {
        self.roots.contains(root)
    }

    /// Whether the key is certainly not there: its file is, and every array on the way to it is
    /// written out and does not have it.
    pub fn is_missing(&self, key: &str) -> bool {
        // A file in a folder is named with the folder: `nested/more.php` is `nested.more`.
        let Some(root) = self
            .roots
            .iter()
            .filter(|root| {
                key == root.as_str()
                    || key
                        .strip_prefix(root.as_str())
                        .is_some_and(|rest| rest.starts_with('.'))
            })
            .max_by_key(|root| root.len())
        else {
            return false;
        };
        let mut prefix = root.to_string();
        for segment in key[root.len()..].split('.').filter(|segment| !segment.is_empty()) {
            if self.open.contains(&prefix) {
                return false;
            }
            prefix.push('.');
            prefix.push_str(segment);
            if !self.by_key.contains_key(&prefix) {
                return true;
            }
        }
        false
    }

    /// Reads the file a stem names: the array it returns.
    pub fn add_file(&mut self, stem: &str, path: &Path, text: &str, locale: Option<&str>) {
        let tree = parse(text).syntax();
        let returned = tree
            .children()
            .filter(|node| node.kind() == RETURN_STATEMENT)
            .filter_map(|node| node.children().next())
            .last();
        let locale = locale.map(str::to_string);
        self.roots.insert(stem.to_string());
        let is_array = returned.as_ref().is_some_and(|node| node.kind() == ARRAY_EXPR);
        self.push(stem.to_string(), is_array, None, path, Span::default(), &locale);
        match returned {
            Some(array) if array.kind() == ARRAY_EXPR => self.read_array(stem, &array, path, &locale),
            _ => {
                self.open.insert(stem.to_string());
            }
        }
    }

    fn push(
        &mut self,
        key: String,
        group: bool,
        value: Option<String>,
        path: &Path,
        span: Span,
        locale: &Option<String>,
    ) {
        let entry = KeyEntry {
            key: key.clone(),
            group,
            value,
            path: path.to_path_buf(),
            span,
            locale: locale.clone(),
        };
        let found = self.by_key.entry(key).or_default();
        if locale.is_none() && !found.is_empty() {
            return;
        }
        found.push(self.entries.len());
        self.entries.push(entry);
    }

    fn read_array(&mut self, prefix: &str, array: &SyntaxNode, path: &Path, locale: &Option<String>) {
        let Some(items) = array_items(array) else {
            self.open.insert(prefix.to_string());
            return;
        };
        for (key, value) in items {
            let Some(Literal::Text(name, span)) = key.as_ref().and_then(literal_of) else {
                self.open.insert(prefix.to_string());
                continue;
            };
            if name.contains('.') {
                self.open.insert(prefix.to_string());
                continue;
            }
            let full = format!("{prefix}.{name}");
            let is_array = value.kind() == ARRAY_EXPR;
            self.push(full.clone(), is_array, short_text(&value), path, span, locale);
            if is_array {
                self.read_array(&full, &value, path, locale);
            } else if value.kind() != LITERAL {
                self.open.insert(full);
            }
        }
    }
}

fn short_text(node: &SyntaxNode) -> Option<String> {
    if node.kind() == ARRAY_EXPR {
        return None;
    }
    let text = node.text().to_string();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (text.len() <= 80).then_some(text)
}
