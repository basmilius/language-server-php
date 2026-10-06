//! Pennant: the features a project defines, by the name `Feature::active('new-api')` and `@feature`
//! check them under. A feature is defined with `Feature::define('name', ...)` in the project's own
//! code, or is a class in `app/Features`, named by its `$name` or else by the class itself.

use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::Section;
use super::source::{Literal, literal_of, resolver_for};
use crate::index::{Index, Origin};
use crate::model::Span;
use crate::test_facts::{argument_expressions, string_value};

#[derive(Clone, Debug, PartialEq)]
pub struct DefinedFeature {
    pub name: String,
    pub path: PathBuf,
    pub span: Span,
}

#[derive(Default)]
pub struct Features {
    pub features: Vec<DefinedFeature>,
    /// A feature is defined under a name the code works out, so a name not seen may still exist.
    pub dynamic: bool,
}

impl Features {
    pub fn find(&self, name: &str) -> Option<&DefinedFeature> {
        self.features.iter().find(|feature| feature.name == name)
    }

    pub fn is_missing(&self, name: &str) -> bool {
        !self.dynamic
            && !self.features.is_empty()
            && !name.is_empty()
            && !name.contains('\\')
            && self.find(name).is_none()
    }
}

const FACADE: &str = "Laravel\\Pennant\\Feature";

impl Section for Features {
    fn build(index: &Index) -> Self {
        let mut found = Features::default();
        if index.class(FACADE).is_none() {
            return found;
        }
        let root = index.framework_root();
        let app = root.join("app");
        let paths: Vec<PathBuf> = index
            .files()
            .filter(|file| file.origin == Origin::Project && file.path.starts_with(&app))
            .map(|file| file.path.clone())
            .collect();
        for path in paths {
            let Some(text) = index.read_text(&path) else {
                continue;
            };
            if !text.contains("define(") {
                continue;
            }
            let tree = php_syntax::parse(&text).syntax();
            for call in tree.descendants().filter(|node| node.kind() == CALL_EXPR) {
                found.read_define(&call, &path);
            }
        }
        let features = app.join("Features");
        for file in index
            .files()
            .filter(|file| file.origin == Origin::Project && file.path.starts_with(&features))
        {
            for class in &file.symbols().classes {
                let named = class
                    .property("name")
                    .and_then(|property| property.default.as_deref())
                    .map(|value| value.trim().trim_matches(['\'', '"']).to_string());
                found.features.push(DefinedFeature {
                    name: named.unwrap_or_else(|| class.name.clone()),
                    path: file.path.clone(),
                    span: class.name_span,
                });
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

impl Features {
    /// `Feature::define('name', fn () => ...)`.
    fn read_define(&mut self, call: &SyntaxNode, path: &Path) {
        let Some(callee) = call
            .children()
            .next()
            .filter(|callee| callee.kind() == SCOPED_ACCESS_EXPR)
        else {
            return;
        };
        let mut names = callee.children().filter(|child| child.kind() == NAME);
        let (class, method) = match (names.next(), names.next()) {
            (Some(class), Some(method)) => (class, method),
            _ => return,
        };
        if !method.text().to_string().eq_ignore_ascii_case("define") {
            return;
        }
        let facade = resolver_for(&callee).resolve_class(&class.text().to_string());
        if !facade.eq_ignore_ascii_case(FACADE) && !facade.eq_ignore_ascii_case("Feature") {
            return;
        }
        let args = argument_expressions(call);
        let Some(first) = args.first() else {
            return;
        };
        match literal_of(first) {
            Some(Literal::Text(name, _)) => {
                let Some((_, span)) = string_value(first) else {
                    return;
                };
                self.features.push(DefinedFeature {
                    name,
                    path: path.to_path_buf(),
                    span,
                });
            }
            // A class feature defined by hand is the class, which `app/Features` may not hold.
            Some(Literal::Class(_)) => self.dynamic = true,
            None => self.dynamic = true,
        }
    }
}
