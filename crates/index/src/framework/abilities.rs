//! The abilities a project declares: `Gate::define('edit-settings', ...)` and the public methods of
//! its policies, which `$user->can('update', $post)` names.

use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;

use super::Section;
use super::source::{Literal, literal_of, tree_of};
use crate::index::{Index, Origin};
use crate::model::{Span, Visibility};
use crate::test_facts::argument_expressions;

const SERVICE_PROVIDER: &str = "Illuminate\\Support\\ServiceProvider";

#[derive(Clone, Debug, PartialEq)]
pub struct Ability {
    pub name: String,
    pub path: PathBuf,
    pub span: Span,
    /// The policy class that declares it, when it is a policy method.
    pub policy: Option<String>,
}

#[derive(Default)]
pub struct Abilities {
    pub abilities: Vec<Ability>,
}

impl Abilities {
    pub fn find_all(&self, name: &str) -> Vec<&Ability> {
        self.abilities.iter().filter(|ability| ability.name == name).collect()
    }
}

impl Section for Abilities {
    fn build(index: &Index) -> Self {
        let mut found = Abilities::default();
        let mut paths: Vec<PathBuf> = Vec::new();
        for provider in index.all_subtypes(SERVICE_PROVIDER) {
            if provider.file.origin == Origin::Project && !paths.contains(&provider.file.path) {
                paths.push(provider.file.path.clone());
            }
        }
        for path in paths {
            let Some(tree) = tree_of(index, &path) else {
                continue;
            };
            for call in tree.descendants().filter(|node| node.kind() == CALL_EXPR) {
                let Some(callee) = call.children().next() else {
                    continue;
                };
                let method = callee.children().filter(|child| child.kind() == NAME).last();
                if !method.is_some_and(|name| name.text() == "define") {
                    continue;
                }
                let receiver = callee
                    .children()
                    .next()
                    .map(|node| node.text().to_string())
                    .unwrap_or_default();
                if !(receiver == "Gate"
                    || receiver.ends_with("\\Gate")
                    || receiver.starts_with("$gate")
                    || receiver == "$this->gate")
                {
                    continue;
                }
                if let Some(Literal::Text(name, span)) = argument_expressions(&call).first().and_then(literal_of) {
                    found.abilities.push(Ability {
                        name,
                        path: path.clone(),
                        span,
                        policy: None,
                    });
                }
            }
        }
        for class in index.class_names() {
            if class.file.origin != Origin::Project || !class.summary.name.ends_with("Policy") {
                continue;
            }
            let Some(loaded) = class.load() else {
                continue;
            };
            for method in &loaded.decl.methods {
                if method.visibility != Visibility::Public
                    || method.is_static
                    || method.name.starts_with("__")
                    || matches!(method.name.as_str(), "before" | "after")
                {
                    continue;
                }
                found.abilities.push(Ability {
                    name: method.name.clone(),
                    path: loaded.file.path.clone(),
                    span: method.name_span,
                    policy: Some(loaded.decl.name.clone()),
                });
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn gates_and_policies_declare_abilities() {
        let index = project(&[
            (
                "vendor/laravel/ServiceProvider.php",
                "<?php namespace Illuminate\\Support; abstract class ServiceProvider {}",
            ),
            (
                "app/Providers/AuthServiceProvider.php",
                "<?php namespace App\\Providers; use Illuminate\\Support\\Facades\\Gate; class AuthServiceProvider extends \\Illuminate\\Support\\ServiceProvider { public function boot() { Gate::define('edit-settings', fn ($user) => true); } }",
            ),
            (
                "app/Policies/PostPolicy.php",
                "<?php namespace App\\Policies; class PostPolicy { public function before() {} public function view($user, $post) {} public function update($user, $post) {} private function hidden() {} }",
            ),
        ]);
        let abilities = index.section::<Abilities>();
        let mut names: Vec<&str> = abilities
            .abilities
            .iter()
            .map(|ability| ability.name.as_str())
            .collect();
        names.sort();
        assert_eq!(names, ["edit-settings", "update", "view"]);
        assert_eq!(
            abilities.find_all("update")[0].policy.as_deref(),
            Some("App\\Policies\\PostPolicy")
        );
    }
}
