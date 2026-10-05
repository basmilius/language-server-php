//! The keys of Laravel's `config/*.php`: `config('app.name')` is the `name` of the array
//! `config/app.php` returns.

use std::collections::HashSet;
use std::path::Path;

use super::Section;
use crate::index::{Index, Origin};

const SERVICE_PROVIDER: &str = "Illuminate\\Support\\ServiceProvider";

pub use super::keytree::KeyEntry as ConfigEntry;
use super::keytree::KeyTree;

#[derive(Default)]
pub struct ConfigKeys {
    tree: KeyTree,
}

impl ConfigKeys {
    pub fn find(&self, key: &str) -> Option<&ConfigEntry> {
        self.tree.find(key)
    }

    /// Whether the key is certainly not there: its file exists, and every array on the way to it is
    /// written out and does not have it. A package may merge more keys into a file, so a file one
    /// merges into is never certain.
    pub fn is_missing(&self, index: &Index, key: &str) -> bool {
        let root = key.split('.').next().unwrap_or("");
        self.tree.is_missing(key)
            && !index.section::<MergedConfigs>().merges(root)
            && !index.section::<RuntimeConfig>().sets(root)
    }

    /// Every key, files and groups included.
    pub fn keys(&self) -> impl Iterator<Item = &ConfigEntry> {
        self.tree.entries.iter()
    }
}

impl Section for ConfigKeys {
    fn build(index: &Index) -> Self {
        let mut keys = ConfigKeys::default();
        let dir = index.framework_root().join("config");
        for path in index.files_below(&dir) {
            if path.extension().is_none_or(|ext| ext != "php") {
                continue;
            }
            let Some(text) = index.read_text(&path) else {
                continue;
            };
            let Ok(relative) = path.strip_prefix(&dir) else {
                continue;
            };
            let stem = relative.with_extension("").to_string_lossy().replace(['/', '\\'], ".");
            keys.tree.add_file(&stem, &path, &text, None);
        }
        keys
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_below(root, path, "config")
    }
}

/// The config files the packages of the project merge more keys into, which the project's own file
/// does not show.
#[derive(Default)]
pub struct MergedConfigs {
    roots: HashSet<String>,
    unknown: bool,
}

impl MergedConfigs {
    pub fn merges(&self, root: &str) -> bool {
        self.unknown || self.roots.contains(root)
    }
}

impl Section for MergedConfigs {
    fn build(index: &Index) -> Self {
        let mut merged = MergedConfigs::default();
        for provider in index.all_subtypes(SERVICE_PROVIDER) {
            if provider.file.origin != Origin::Vendor || provider.decl.name.starts_with("Illuminate\\") {
                continue;
            }
            let Some(text) = index.read_text(&provider.file.path) else {
                continue;
            };
            merged.scan(&text);
        }
        merged
    }

    fn depends_on(_: &Path, _: &Path) -> bool {
        false
    }
}

impl MergedConfigs {
    fn scan(&mut self, text: &str) {
        let mut rest = text;
        while let Some(at) = rest.find("mergeConfigFrom(") {
            rest = &rest[at + "mergeConfigFrom(".len()..];
            let call = rest.split(';').next().unwrap_or("");
            let name = call
                .rsplit_once(',')
                .map(|(_, tail)| tail.trim().trim_end_matches(')').trim())
                .filter(|tail| tail.starts_with(['\'', '"']) && tail.ends_with(['\'', '"']))
                .map(|tail| tail.trim_matches(['\'', '"']).to_string());
            match name {
                Some(name) => {
                    self.roots.insert(name);
                }
                None => self.unknown = true,
            }
        }
    }
}

/// The config roots the project writes while it runs: `config(['x.y' => 1])` and `Config::set()`.
#[derive(Default)]
pub struct RuntimeConfig {
    roots: HashSet<String>,
    /// A write whose key is not written out.
    unknown: bool,
}

impl RuntimeConfig {
    pub fn sets(&self, root: &str) -> bool {
        self.unknown || self.roots.contains(root)
    }
}

impl Section for RuntimeConfig {
    fn build(index: &Index) -> Self {
        let mut found = RuntimeConfig::default();
        let paths: Vec<std::path::PathBuf> = index
            .files()
            .filter(|file| file.origin == Origin::Project)
            .map(|file| file.path.clone())
            .collect();
        for path in paths {
            let Some(text) = index.read_text(&path) else {
                continue;
            };
            if !(text.contains("config([") || text.contains("Config::set(") || text.contains("config()->set(")) {
                continue;
            }
            let tree = php_syntax::parse(&text).syntax();
            for call in tree
                .descendants()
                .filter(|node| node.kind() == php_syntax::SyntaxKind::CALL_EXPR)
            {
                found.read(&call);
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

impl RuntimeConfig {
    fn read(&mut self, call: &php_syntax::SyntaxNode) {
        use php_syntax::SyntaxKind::*;
        let Some(callee) = call.children().next() else {
            return;
        };
        let text = callee.text().to_string().replace(char::is_whitespace, "");
        let writes = text == "config" || text.ends_with("Config::set") || text.ends_with("config()->set");
        if !writes {
            return;
        }
        let Some(first) = crate::test_facts::argument_expressions(call).into_iter().next() else {
            return;
        };
        let note = |roots: &mut HashSet<String>, unknown: &mut bool, node: &php_syntax::SyntaxNode| {
            match super::source::literal_of(node) {
                Some(super::source::Literal::Text(key, _)) => {
                    roots.insert(key.split('.').next().unwrap_or("").to_string());
                }
                _ => *unknown = true,
            }
        };
        if first.kind() == ARRAY_EXPR {
            match super::source::array_items(&first) {
                Some(items) => {
                    for (key, _) in items {
                        match key {
                            Some(key) => note(&mut self.roots, &mut self.unknown, &key),
                            None => self.unknown = true,
                        }
                    }
                }
                None => self.unknown = true,
            }
        } else if text != "config" {
            note(&mut self.roots, &mut self.unknown, &first);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    const APP: &str = r#"<?php
return [
    'name' => env('APP_NAME', 'Laravel'),
    'debug' => (bool) env('APP_DEBUG', false),
    'locale' => 'en',
    'providers' => [
        App\Providers\AppServiceProvider::class,
    ],
    'aliases' => [
        'Foo' => Bar::class,
    ],
    'nested' => [
        'a' => ['b' => 1],
        'computed' => array_merge(['x' => 1], []),
    ],
];
"#;

    fn keys(extra: &[(&str, &str)]) -> (crate::index::Index, std::sync::Arc<ConfigKeys>) {
        let mut files = vec![
            ("config/app.php", APP),
            ("config/nested/more.php", "<?php return ['k' => 'v'];"),
        ];
        files.extend_from_slice(extra);
        let index = project(&files);
        let keys = index.section::<ConfigKeys>();
        (index, keys)
    }

    #[test]
    fn reads_the_keys_of_the_config_files() {
        let (_, keys) = keys(&[]);
        assert_eq!(
            keys.find("app.name").and_then(|entry| entry.value.clone()).as_deref(),
            Some("env('APP_NAME', 'Laravel')")
        );
        assert!(keys.find("app").is_some_and(|entry| entry.group));
        assert!(keys.find("app.aliases.Foo").is_some());
        assert!(keys.find("app.nested.a.b").is_some());
        assert!(keys.find("nested.more.k").is_some(), "a folder is part of the key");
        assert_eq!(
            keys.find("app.locale").map(|entry| entry.span.end - entry.span.start),
            Some(6)
        );
    }

    #[test]
    fn a_key_is_missing_only_when_every_array_on_the_way_is_written_out() {
        let (index, keys) = keys(&[]);
        assert!(keys.is_missing(&index, "app.nam"));
        assert!(
            keys.is_missing(&index, "app.locale.deeper"),
            "below a scalar nothing exists"
        );
        assert!(keys.is_missing(&index, "app.nested.a.c"));
        assert!(!keys.is_missing(&index, "app.name"));
        assert!(!keys.is_missing(&index, "app.providers.0"), "a list is not enumerated");
        assert!(
            !keys.is_missing(&index, "app.nested.computed.y"),
            "a computed value may hold anything"
        );
        assert!(
            !keys.is_missing(&index, "other.name"),
            "a file that is not there may come from a package"
        );
        assert!(!keys.is_missing(&index, "app"));
    }

    #[test]
    fn a_file_the_project_writes_while_it_runs_is_never_certain() {
        let (index, keys) = keys(&[
            (
                "vendor/laravel/ServiceProvider.php",
                "<?php namespace Illuminate\\Support; abstract class ServiceProvider {}",
            ),
            (
                "app/Boot.php",
                "<?php class Boot { public function run() { config(['app.extra' => 1]); } }",
            ),
        ]);
        assert!(!keys.is_missing(&index, "app.nam"), "the project writes into app");
        assert!(
            keys.is_missing(&index, "nested.more.nope"),
            "nothing writes into the nested file"
        );
    }

    #[test]
    fn a_file_a_package_merges_into_is_never_certain() {
        let (index, keys) = keys(&[
            (
                "vendor/acme/Provider.php",
                "<?php namespace Acme; class Provider extends \\Illuminate\\Support\\ServiceProvider { public function register() { $this->mergeConfigFrom(__DIR__.'/c.php', 'app'); } }",
            ),
            (
                "vendor/laravel/ServiceProvider.php",
                "<?php namespace Illuminate\\Support; abstract class ServiceProvider {}",
            ),
        ]);
        assert!(!keys.is_missing(&index, "app.nam"));
    }
}
