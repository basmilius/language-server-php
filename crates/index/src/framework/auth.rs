//! The model that logs in: the `model` of the first provider in `config/auth.php`.

use std::path::Path;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::Section;
use super::source::{Literal, array_items, literal_of};
use crate::index::Index;
use crate::types::Name;

#[derive(Default)]
pub struct UserModel {
    pub class: Option<Name>,
}

impl UserModel {
    /// The class of the user, when the configuration names it.
    pub fn class_of(index: &Index) -> Option<Name> {
        index.section::<UserModel>().class.clone()
    }
}

impl Section for UserModel {
    fn build(index: &Index) -> Self {
        let path = index.framework_root().join("config").join("auth.php");
        let class = index.read_text(&path).and_then(|text| read(&parse(&text).syntax()));
        UserModel { class }
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        path == root.join("config").join("auth.php")
    }
}

/// The class `model` names in the first provider, wherever the `::class` stands in the value.
fn read(tree: &SyntaxNode) -> Option<Name> {
    let config = tree
        .children()
        .filter(|node| node.kind() == RETURN_STATEMENT)
        .filter_map(|node| node.children().next())
        .find(|node| node.kind() == ARRAY_EXPR)?;
    let providers = value_of(&config, "providers")?;
    let (_, first) = array_items(&providers)?.into_iter().next()?;
    let model = value_of(&first, "model")?;
    let constant = model
        .descendants()
        .find(|node| node.kind() == SCOPED_ACCESS_EXPR && node.text().to_string().ends_with("::class"))?;
    match literal_of(&constant)? {
        Literal::Class(class) => Some(class),
        Literal::Text(..) => None,
    }
}

fn value_of(array: &SyntaxNode, key: &str) -> Option<SyntaxNode> {
    array_items(array)?
        .into_iter()
        .find_map(|(name, value)| match name.as_ref().and_then(literal_of) {
            Some(Literal::Text(text, _)) if text == key => Some(value),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn reads_the_model_of_the_first_provider() {
        let index = project(&[(
            "config/auth.php",
            "<?php\nuse App\\Models\\User;\nreturn ['guards' => [], 'providers' => ['users' => ['driver' => 'eloquent', 'model' => env('AUTH_MODEL', User::class)]]];",
        )]);
        assert_eq!(index.section::<UserModel>().class.as_deref(), Some("App\\Models\\User"));
        let plain = project(&[(
            "config/auth.php",
            "<?php return ['providers' => ['users' => ['model' => \\App\\Account::class]]];",
        )]);
        assert_eq!(plain.section::<UserModel>().class.as_deref(), Some("App\\Account"));
        let none = project(&[(
            "config/auth.php",
            "<?php return ['providers' => ['users' => ['driver' => 'database']]];",
        )]);
        assert_eq!(none.section::<UserModel>().class, None);
    }
}
