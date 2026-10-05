//! Translations: `__('messages.welcome')` is the `welcome` of `lang/<locale>/messages.php`, and
//! `__('Welcome')` is a key of `lang/<locale>.json`.

use std::collections::HashSet;
use std::path::Path;

use serde_json::Value;

use super::Section;
use super::keytree::{KeyEntry, KeyTree};
use crate::index::Index;
use crate::model::Span;

#[derive(Default)]
pub struct Translations {
    tree: KeyTree,
    json: Vec<KeyEntry>,
    json_keys: HashSet<String>,
}

impl Translations {
    /// Where a key is written, in every locale that has it.
    pub fn find(&self, key: &str) -> Vec<&KeyEntry> {
        let mut found = self.tree.find_all(key);
        found.extend(self.json.iter().filter(|entry| entry.key == key));
        found
    }

    /// Every key, from the PHP files and the JSON files.
    pub fn keys(&self) -> impl Iterator<Item = &KeyEntry> {
        self.tree.entries.iter().chain(self.json.iter())
    }

    /// Whether the key is certainly not there. A key written in words is its own translation, so
    /// only a key that starts with the name of a group file, in a language that has it, can be
    /// wrong.
    pub fn is_missing(&self, key: &str) -> bool {
        if key.contains("::") || key.contains(char::is_whitespace) || !key.contains('.') {
            return false;
        }
        let group = key.split('.').next().unwrap_or("");
        self.tree.has_root(group) && !self.json_keys.contains(key) && self.tree.is_missing(key)
    }
}

impl Section for Translations {
    fn build(index: &Index) -> Self {
        let mut translations = Translations::default();
        let root = index.framework_root();
        for dir in [root.join("lang"), root.join("resources").join("lang")] {
            for path in index.files_below(&dir) {
                let Ok(relative) = path.strip_prefix(&dir) else {
                    continue;
                };
                let parts: Vec<String> = relative
                    .iter()
                    .map(|part| part.to_string_lossy().into_owned())
                    .collect();
                if parts.first().is_none_or(|first| first == "vendor") {
                    continue;
                }
                let Some(text) = index.read_text(&path) else {
                    continue;
                };
                match (parts.len(), path.extension().and_then(|ext| ext.to_str())) {
                    (1, Some("json")) => {
                        let locale = path
                            .file_stem()
                            .map(|stem| stem.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        for entry in json_entries(&path, &text, &locale) {
                            translations.json_keys.insert(entry.key.clone());
                            translations.json.push(entry);
                        }
                    }
                    (count, Some("php")) if count >= 2 => {
                        let stem = parts[1..].join("/");
                        let stem = stem.strip_suffix(".php").unwrap_or(&stem);
                        translations.tree.add_file(stem, &path, &text, Some(&parts[0]));
                    }
                    _ => {}
                }
            }
        }
        translations
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_below(root, path, "lang") || super::is_below(root, path, "resources/lang")
    }
}

fn json_entries(path: &Path, text: &str, locale: &str) -> Vec<KeyEntry> {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    map.iter()
        .map(|(key, value)| {
            let start = text.find(&format!("\"{key}\"")).map_or(0, |at| at + 1) as u32;
            KeyEntry {
                key: key.clone(),
                group: false,
                value: value.as_str().map(str::to_string),
                path: path.to_path_buf(),
                span: Span {
                    start,
                    end: start + key.len() as u32,
                },
                locale: Some(locale.to_string()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    fn index() -> Index {
        project(&[
            (
                "lang/en/messages.php",
                "<?php return ['welcome' => 'Welcome', 'nested' => ['deep' => 'x']];",
            ),
            ("lang/nl/messages.php", "<?php return ['welcome' => 'Welkom'];"),
            ("lang/en/admin/users.php", "<?php return ['title' => 'Users'];"),
            (
                "lang/en.json",
                "{\n  \"Welcome back.\": \"Welcome back.\",\n  \"auth.custom\": \"x\"\n}",
            ),
            ("lang/vendor/pkg/en/x.php", "<?php return ['k' => 'v'];"),
        ])
    }

    #[test]
    fn reads_the_group_files_and_the_json_files_of_every_locale() {
        let index = index();
        let translations = index.section::<Translations>();
        assert_eq!(translations.find("messages.welcome").len(), 2);
        assert_eq!(translations.find("messages.nested.deep").len(), 1);
        assert_eq!(translations.find("admin/users.title").len(), 1);
        assert_eq!(translations.find("Welcome back.").len(), 1);
        assert!(translations.find("pkg::x.k").is_empty());
        let json = translations.find("Welcome back.")[0];
        let text = "{\n  \"Welcome back.\": \"Welcome back.\",";
        assert_eq!(&text[json.span.start as usize..json.span.end as usize], "Welcome back.");
    }

    #[test]
    fn only_a_key_of_a_known_group_can_be_wrong() {
        let index = index();
        let translations = index.section::<Translations>();
        assert!(translations.is_missing("messages.welcom"));
        assert!(translations.is_missing("messages.nested.nope"));
        assert!(!translations.is_missing("messages.welcome"));
        assert!(
            !translations.is_missing("Welcome back."),
            "words are their own translation"
        );
        assert!(!translations.is_missing("Hello. How are you?"));
        assert!(!translations.is_missing("other.key"), "no such group file");
        assert!(!translations.is_missing("auth.custom"), "a json key may hold dots");
        assert!(!translations.is_missing("pkg::messages.key"));
    }
}
