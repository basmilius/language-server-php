//! The global class aliases Laravel registers at start: `DB` is `Illuminate\Support\Facades\DB`.
//! They are the ones `Facade::defaultAliases()` lists and the `aliases` of `config/app.php`, and a
//! file that writes `use DB;` or `\DB::table()` means the facade.

use std::collections::HashMap;
use std::path::Path;

use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::Section;
use super::source::{Literal, array_items, literal_of, method_at, tree_of};
use crate::index::Index;
use crate::types::Name;

const FACADE: &str = "Illuminate\\Support\\Facades\\Facade";

#[derive(Default)]
pub struct ClassAliases {
    by_alias: HashMap<String, Name>,
}

impl ClassAliases {
    pub fn target(&self, alias: &str) -> Option<&Name> {
        self.by_alias.get(&alias.to_ascii_lowercase())
    }

    fn read(&mut self, array: &SyntaxNode) {
        for (key, value) in array_items(array).unwrap_or_default() {
            let Some(Literal::Text(alias, _)) = key.as_ref().and_then(literal_of) else {
                continue;
            };
            if let Some(Literal::Class(class)) = literal_of(&value) {
                self.by_alias.insert(alias.to_ascii_lowercase(), class);
            }
        }
    }
}

impl Section for ClassAliases {
    fn build(index: &Index) -> Self {
        let mut aliases = ClassAliases::default();
        if let Some(facade) = index.class(FACADE) {
            if let Some(method) = facade.decl.method("defaultAliases") {
                if let Some(tree) = tree_of(index, &facade.file.path) {
                    if let Some(body) = method_at(&tree, method.name_span.start) {
                        for array in body.descendants().filter(|node| node.kind() == ARRAY_EXPR) {
                            aliases.read(&array);
                        }
                    }
                }
            }
        }
        let config = index.framework_root().join("config").join("app.php");
        if let Some(tree) = tree_of(index, &config) {
            for item in tree.descendants().filter(|node| node.kind() == ARRAY_ITEM) {
                let is_aliases = item
                    .children()
                    .next()
                    .and_then(|key| crate::test_facts::string_value(&key))
                    .is_some_and(|(key, _)| key == "aliases");
                if !is_aliases {
                    continue;
                }
                for array in item.descendants().filter(|node| node.kind() == ARRAY_EXPR) {
                    aliases.read(&array);
                }
            }
        }
        aliases
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        path == root.join("config").join("app.php")
    }
}

#[cfg(test)]
mod tests {
    use crate::framework::testing::project;

    #[test]
    fn a_global_alias_is_the_facade_it_names() {
        let index = project(&[
            (
                "vendor/laravel/Facade.php",
                "<?php\nnamespace Illuminate\\Support\\Facades;\nuse Illuminate\\Support\\Collection;\nabstract class Facade {\n    public static function defaultAliases() {\n        return new Collection(['DB' => DB::class, 'Str' => \\Illuminate\\Support\\Str::class]);\n    }\n}\nclass DB extends Facade {}\n",
            ),
            (
                "vendor/laravel/Str.php",
                "<?php namespace Illuminate\\Support; class Str {}",
            ),
            (
                "config/app.php",
                "<?php\nuse Illuminate\\Support\\Facades\\Facade;\nreturn ['aliases' => Facade::defaultAliases()->merge(['Theme' => App\\Facades\\Theme::class])->toArray()];\n",
            ),
            ("app/Facades/Theme.php", "<?php namespace App\\Facades; class Theme {}"),
        ]);
        let name = |alias: &str| index.class(alias).map(|class| class.decl.name.clone());
        assert_eq!(name("DB").as_deref(), Some("Illuminate\\Support\\Facades\\DB"));
        assert_eq!(name("\\Str").as_deref(), Some("Illuminate\\Support\\Str"));
        assert_eq!(name("Theme").as_deref(), Some("App\\Facades\\Theme"));
        assert!(index.class("Nope").is_none());
    }
}
