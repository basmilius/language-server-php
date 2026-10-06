//! Filament: the model each class of a panel works on, which the names of its columns, fields and
//! entries (`TextColumn::make('author.name')`) are attributes of. A resource's model is its `$model`,
//! or the model named after it; a class the resource calls statically (`ArticlesTable::configure()`)
//! and a page whose `$resource` names it work on the same model.

use std::collections::HashMap;
use std::path::Path;

use php_syntax::SyntaxKind::*;

use super::Section;
use super::source::{class_in_attribute, tree_of};
use crate::extract::resolver_at;
use crate::index::{Index, Origin};
use crate::types::Name;

const RESOURCE: &str = "Filament\\Resources\\Resource";

#[derive(Default)]
pub struct FilamentModels {
    /// By lowercase class; `None` where two resources claim the class.
    by_class: HashMap<String, Option<Name>>,
}

impl FilamentModels {
    pub fn model_of(&self, class: &str) -> Option<&Name> {
        self.by_class.get(&class.to_ascii_lowercase())?.as_ref()
    }

    fn give(&mut self, class: &str, model: &str) {
        let key = class.to_ascii_lowercase();
        match self.by_class.get(&key) {
            Some(Some(known)) if !known.eq_ignore_ascii_case(model) => {
                self.by_class.insert(key, None);
            }
            Some(_) => {}
            None => {
                self.by_class.insert(key, Some(model.to_string()));
            }
        }
    }
}

impl Section for FilamentModels {
    fn build(index: &Index) -> Self {
        let mut found = FilamentModels::default();
        if index.class(RESOURCE).is_none() {
            return found;
        }
        let mut resources: Vec<(Name, Name)> = Vec::new();
        for resource in index.all_subtypes(RESOURCE) {
            if resource.file.origin != Origin::Project {
                continue;
            }
            let model = resource
                .decl
                .property("model")
                .and_then(|property| property.default.as_deref())
                .and_then(|value| class_in_attribute(index, resource, value))
                .or_else(|| {
                    let short = resource.decl.name.rsplit('\\').next()?.strip_suffix("Resource")?;
                    let conventional = format!("App\\Models\\{short}");
                    index.class(&conventional).map(|_| conventional)
                });
            let Some(model) = model else {
                continue;
            };
            found.give(&resource.decl.name, &model);
            resources.push((resource.decl.name.clone(), model.clone()));
            let Some(tree) = tree_of(index, &resource.file.path) else {
                continue;
            };
            let resolver = resolver_at(&tree, resource.decl.span.start);
            for access in tree.descendants().filter(|node| node.kind() == SCOPED_ACCESS_EXPR) {
                let Some(name) = access.children().find(|child| child.kind() == NAME) else {
                    continue;
                };
                let class = resolver.resolve_class(&name.text().to_string());
                if index
                    .class(&class)
                    .is_some_and(|found| found.file.origin == Origin::Project)
                    && !class.eq_ignore_ascii_case(&model)
                {
                    found.give(&class, &model);
                }
            }
        }
        for file in index.files().filter(|file| file.origin == Origin::Project) {
            for decl in &file.symbols().classes {
                let Some(value) = decl
                    .property("resource")
                    .and_then(|property| property.default.as_deref())
                else {
                    continue;
                };
                let Some(class) = index.class(&decl.name) else {
                    continue;
                };
                let Some(resource) = class_in_attribute(index, class, value) else {
                    continue;
                };
                if let Some((_, model)) = resources.iter().find(|(name, _)| name.eq_ignore_ascii_case(&resource)) {
                    found.give(&decl.name, model);
                }
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}
