//! The serialization groups of Symfony's serializer, which API Platform reads too: the groups a
//! `#[Groups]` on a property or a method puts it in, and which a normalization context then names.

use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;

use crate::framework::Section;
use crate::framework::source::tree_of;
use crate::index::{Index, Origin};
use crate::model::Span;
use crate::test_facts::string_value;

pub const GROUPS: [&str; 2] = [
    "Symfony\\Component\\Serializer\\Attribute\\Groups",
    "Symfony\\Component\\Serializer\\Annotation\\Groups",
];

#[derive(Clone, Debug, PartialEq)]
pub struct GroupPlace {
    pub name: String,
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Default)]
pub struct SerializerGroups {
    pub places: Vec<GroupPlace>,
    /// Groups may also be mapped in YAML or XML, which is not read.
    pub mapped_elsewhere: bool,
}

impl SerializerGroups {
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.places.iter().map(|place| place.name.as_str()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn is_missing(&self, name: &str) -> bool {
        !self.mapped_elsewhere && !self.places.is_empty() && self.places.iter().all(|place| place.name != name)
    }
}

fn is_groups(name: &str) -> bool {
    GROUPS.iter().any(|groups| groups.eq_ignore_ascii_case(name))
}

impl Section for SerializerGroups {
    fn build(index: &Index) -> Self {
        let mut found = SerializerGroups::default();
        let root = index.framework_root();
        found.mapped_elsewhere = ["config/serializer", "config/api_platform"]
            .iter()
            .any(|dir| !index.files_below(&root.join(dir)).is_empty());
        let paths: Vec<PathBuf> = index
            .files()
            .filter(|file| file.origin == Origin::Project)
            .filter(|file| {
                file.symbols().classes.iter().any(|class| {
                    class
                        .properties
                        .iter()
                        .any(|property| property.attributes.iter().any(|attribute| is_groups(&attribute.name)))
                        || class
                            .methods
                            .iter()
                            .any(|method| method.attributes.iter().any(|attribute| is_groups(&attribute.name)))
                        || class.attributes.iter().any(|attribute| is_groups(&attribute.name))
                })
            })
            .map(|file| file.path.clone())
            .collect();
        for path in paths {
            let Some(tree) = tree_of(index, &path) else {
                continue;
            };
            for attribute in tree.descendants().filter(|node| node.kind() == ATTRIBUTE) {
                let Some(name) = attribute.children().find(|child| child.kind() == NAME) else {
                    continue;
                };
                let resolver = crate::framework::source::resolver_for(&attribute);
                if !is_groups(&resolver.resolve_class(&name.text().to_string())) {
                    continue;
                }
                for literal in attribute.descendants().filter(|node| node.kind() == LITERAL) {
                    if let Some((group, span)) = string_value(&literal) {
                        found.places.push(GroupPlace {
                            name: group,
                            path: path.clone(),
                            span,
                        });
                    }
                }
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_project_php(root, path) || path.starts_with(root.join("config"))
    }
}
