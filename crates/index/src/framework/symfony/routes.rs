//! Symfony's routes: `#[Route]` attributes, `config/routes*.yaml` and the PHP route configuration, and
//! the files a bundle's route import points at.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;

use super::{config_files, is_config, yaml_of};
use crate::framework::Section;
use crate::framework::source::{Literal, literal_of, tree_of};
use crate::framework::yaml::Node;
use crate::index::{Index, Origin};
use crate::model::{Attribute, Span};
use crate::test_facts::argument_expressions;

#[derive(Clone, Debug, PartialEq)]
pub struct SfRoute {
    pub name: String,
    pub path: Option<String>,
    pub file: PathBuf,
    /// The method of an attribute route, the name of a configured one.
    pub span: Span,
}

#[derive(Default)]
pub struct SfRoutes {
    pub routes: Vec<SfRoute>,
    by_name: HashMap<String, usize>,
    /// Something registers routes the reading could not follow.
    pub incomplete: bool,
}

impl SfRoutes {
    pub fn find(&self, name: &str) -> Option<&SfRoute> {
        self.by_name.get(name).map(|at| &self.routes[*at])
    }

    /// Whether the route is certainly not there: every source of routes was read.
    pub fn is_missing(&self, name: &str) -> bool {
        !self.incomplete && !name.is_empty() && self.find(name).is_none()
    }

    fn push(&mut self, route: SfRoute) {
        if self.by_name.contains_key(&route.name) {
            return;
        }
        self.by_name.insert(route.name.clone(), self.routes.len());
        self.routes.push(route);
    }
}

fn is_route_attribute(attribute: &Attribute) -> bool {
    let name = attribute.name.as_str();
    name.starts_with("Symfony\\Component\\Routing\\") && name.ends_with("\\Route")
}

fn unquote(text: &str) -> Option<String> {
    let text = text.trim();
    let quoted = text.len() >= 2
        && (text.starts_with('\'') && text.ends_with('\'') || text.starts_with('"') && text.ends_with('"'));
    quoted.then(|| text[1..text.len() - 1].to_string())
}

/// The `path` and `name` of a `#[Route]`: named arguments, or the first two positional ones.
fn route_args(attribute: &Attribute) -> (Option<String>, Option<String>, bool) {
    let mut positional = attribute.args.iter().filter(|arg| arg.name.is_none());
    let first = positional.next();
    let second = positional.next();
    fn pick<'a>(
        attribute: &'a Attribute,
        name: &str,
        fallback: Option<&'a crate::model::AttributeArg>,
    ) -> Option<&'a crate::model::AttributeArg> {
        attribute
            .args
            .iter()
            .find(|arg| arg.name.as_deref() == Some(name))
            .or(fallback)
    }
    let path = pick(attribute, "path", first);
    let name = pick(attribute, "name", second);
    let dynamic = name.is_some_and(|arg| unquote(&arg.value).is_none());
    (
        path.and_then(|arg| unquote(&arg.value)),
        name.and_then(|arg| unquote(&arg.value)),
        dynamic,
    )
}

impl Section for SfRoutes {
    fn build(index: &Index) -> Self {
        let mut routes = SfRoutes::default();
        routes.read_attributes(index);
        for path in config_files(index, &["yaml", "yml"]) {
            if !is_route_file(index.framework_root(), &path) {
                continue;
            }
            if let Some((_, root)) = yaml_of(index, &path) {
                routes.read_yaml(index, &path, &root, "");
            }
        }
        for path in config_files(index, &["php"]) {
            if is_route_file(index.framework_root(), &path) {
                routes.read_php(index, &path, "");
            }
        }
        // A kernel that loads its routes itself may import what no file names (the routes of the
        // plugins it finds).
        let kernel = index.framework_root().join("src").join("Kernel.php");
        if index
            .read_text(&kernel)
            .is_some_and(|text| text.contains("function configureRoutes"))
        {
            routes.incomplete = true;
        }
        routes
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        path == root.join("src").join("Kernel.php") || is_config(root, path)
    }
}

/// The name Symfony gives a route that has none: the class and the method, in lower case, without
/// `controller_`, `bundle_` and a trailing `action`.
fn default_name(class: &str, method: &str) -> String {
    let mut name = format!("{}_{method}", class.replace('\\', "_")).to_lowercase();
    for word in ["bundle_", "controller_"] {
        name = name.replace(word, "_");
    }
    if let Some(at) = name.rfind("action") {
        let rest = &name[at + "action".len()..];
        let digits = rest
            .strip_prefix('_')
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
        if rest.is_empty() || digits {
            name = format!("{}{rest}", &name[..at]);
        }
    }
    name.replace("__", "_")
}

fn is_route_file(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative.starts_with("config/routes")
        || relative.starts_with("config/routes.yaml")
        || relative.starts_with("config/routes.php")
        || relative.starts_with("config/routes.yml")
}

impl SfRoutes {
    fn read_attributes(&mut self, index: &Index) {
        for class in index.class_names() {
            if class.file.origin != Origin::Project {
                continue;
            }
            let Some(loaded) = class.load() else {
                continue;
            };
            let decl = loaded.decl;
            let has_routes = decl.attributes.iter().any(is_route_attribute)
                || decl
                    .methods
                    .iter()
                    .any(|method| method.attributes.iter().any(is_route_attribute));
            if !has_routes {
                continue;
            }
            let (class_path, class_name, _) = decl
                .attributes
                .iter()
                .find(|attribute| is_route_attribute(attribute))
                .map_or((None, None, false), route_args);
            for method in &decl.methods {
                for attribute in method
                    .attributes
                    .iter()
                    .filter(|attribute| is_route_attribute(attribute))
                {
                    let (path, name, dynamic) = route_args(attribute);
                    if dynamic {
                        self.incomplete = true;
                    }
                    let name = name.unwrap_or_else(|| default_name(&decl.name, &method.name));
                    let full_path = match (&class_path, path) {
                        (Some(prefix), Some(path)) => Some(format!("{prefix}{path}")),
                        (None, path) => path,
                        (Some(prefix), None) => Some(prefix.clone()),
                    };
                    self.push(SfRoute {
                        name: format!("{}{name}", class_name.clone().unwrap_or_default()),
                        path: full_path,
                        file: loaded.file.path.clone(),
                        span: method.name_span,
                    });
                }
            }
            if decl
                .methods
                .iter()
                .all(|method| !method.attributes.iter().any(is_route_attribute))
            {
                // A class-level route on an invokable controller.
                if let (Some(name), Some(attribute)) = (
                    class_name,
                    decl.attributes.iter().find(|attribute| is_route_attribute(attribute)),
                ) {
                    let (path, ..) = route_args(attribute);
                    self.push(SfRoute {
                        name,
                        path,
                        file: loaded.file.path.clone(),
                        span: decl.name_span,
                    });
                }
            }
        }
    }

    fn read_yaml(&mut self, index: &Index, path: &Path, root: &Node, prefix: &str) {
        let mut documents = vec![root];
        for (name, _, value) in root.entries() {
            if name.starts_with("when@") {
                documents.push(value);
            }
        }
        for document in documents {
            for (name, span, value) in document.entries() {
                if name.starts_with("when@") {
                    continue;
                }
                if let Some(resource) = value.get("resource").and_then(Node::as_str) {
                    let name_prefix = value.get("name_prefix").and_then(Node::as_str).unwrap_or("");
                    let kind = value.get("type").and_then(Node::as_str).unwrap_or("");
                    self.follow(index, path, resource, kind, &format!("{prefix}{name_prefix}"));
                } else if value.get("path").is_some() || value.get("controller").is_some() {
                    self.push(SfRoute {
                        name: format!("{prefix}{name}"),
                        path: value.get("path").and_then(Node::as_str).map(str::to_string),
                        file: path.to_path_buf(),
                        span,
                    });
                }
            }
        }
    }

    /// An import: the project's own controllers are the attributes already read; a file of a bundle is
    /// read; anything else is not known.
    fn follow(&mut self, index: &Index, from: &Path, resource: &str, kind: &str, prefix: &str) {
        let project_attributes = resource == "routing.controllers"
            || (matches!(kind, "" | "attribute" | "annotation" | "attributes" | "directory")
                && !resource.starts_with('@')
                && (resource.starts_with("../src")
                    || resource.starts_with("../../src")
                    || resource.starts_with("src")));
        if project_attributes {
            return;
        }
        if kind == "service" && resource.contains("route_loader.logout") {
            self.read_logout_routes(index);
            return;
        }
        let Some(file) = self.resolve(index, from, resource) else {
            self.incomplete = true;
            return;
        };
        match file.extension().and_then(|ext| ext.to_str()) {
            Some("yaml" | "yml") => match yaml_of(index, &file) {
                Some((_, root)) => self.read_yaml(index, &file, &root, prefix),
                None => self.incomplete = true,
            },
            Some("php") => self.read_php(index, &file, prefix),
            Some("xml") => match index.read_text(&file) {
                Some(text) => self.read_xml(&file, &text, prefix),
                None => self.incomplete = true,
            },
            _ => self.incomplete = true,
        }
    }

    /// `@FrameworkBundle/Resources/config/routing/errors.php` is a file of the bundle that class
    /// `FrameworkBundle` of `config/bundles.php` is in.
    fn resolve(&self, index: &Index, from: &Path, resource: &str) -> Option<PathBuf> {
        if let Some(rest) = resource.strip_prefix('@') {
            let (bundle, relative) = rest.split_once('/')?;
            let bundles = index.read_text(&index.framework_root().join("config").join("bundles.php"))?;
            for line in bundles.lines() {
                let Some((class, _)) = line.split_once("::class") else {
                    continue;
                };
                let class = class.trim().trim_start_matches('\\');
                if class.rsplit('\\').next() != Some(bundle) {
                    continue;
                }
                let found = index.class(class)?;
                let dir = found.file.path.parent()?;
                for candidate in [dir.join(relative), dir.join(relative.trim_start_matches("Resources/"))] {
                    if index.file_exists(&candidate) {
                        return Some(candidate);
                    }
                }
            }
            return None;
        }
        let candidate = from.parent()?.join(resource);
        index.file_exists(&candidate).then_some(candidate)
    }

    fn read_php(&mut self, index: &Index, path: &Path, prefix: &str) {
        let Some(tree) = tree_of(index, path) else {
            return;
        };
        for call in tree.descendants().filter(|node| node.kind() == CALL_EXPR) {
            let Some(callee) = call
                .children()
                .next()
                .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
            else {
                continue;
            };
            let Some(method) = callee.children().find(|child| child.kind() == NAME) else {
                continue;
            };
            let args = argument_expressions(&call);
            match method.text().to_string().as_str() {
                "add" | "route" => {
                    if let Some(Literal::Text(name, span)) = args.first().and_then(literal_of) {
                        let route_path = match args.get(1).and_then(literal_of) {
                            Some(Literal::Text(text, _)) => Some(text),
                            _ => None,
                        };
                        self.push(SfRoute {
                            name: format!("{prefix}{name}"),
                            path: route_path,
                            file: path.to_path_buf(),
                            span,
                        });
                    } else {
                        self.incomplete = true;
                    }
                }
                "import" => self.incomplete = true,
                _ => {}
            }
        }
    }

    fn read_xml(&mut self, path: &Path, text: &str, prefix: &str) {
        let mut rest = 0;
        while let Some(at) = text[rest..].find("<route ") {
            let start = rest + at;
            let end = text[start..].find('>').map_or(text.len(), |close| start + close);
            let tag = &text[start..end];
            if let Some(id_at) = tag.find("id=\"") {
                let from = start + id_at + 4;
                if let Some(length) = text[from..].find('"') {
                    self.push(SfRoute {
                        name: format!("{prefix}{}", &text[from..from + length]),
                        path: None,
                        file: path.to_path_buf(),
                        span: Span {
                            start: from as u32,
                            end: (from + length) as u32,
                        },
                    });
                }
            }
            rest = end;
        }
    }

    /// The logout route has the name the firewall's `logout.path` gives it.
    fn read_logout_routes(&mut self, index: &Index) {
        for path in config_files(index, &["yaml", "yml"]) {
            let Some((_, root)) = yaml_of(index, &path) else {
                continue;
            };
            let Some(firewalls) = root
                .get("security")
                .and_then(|security| security.get("firewalls"))
                .or_else(|| {
                    root.get("when@dev")
                        .and_then(|dev| dev.get("security"))
                        .and_then(|s| s.get("firewalls"))
                })
            else {
                continue;
            };
            for (_, _, firewall) in firewalls.entries() {
                if let Some(logout) = firewall.get("logout") {
                    let target = logout.get("path").and_then(Node::as_str).unwrap_or("app_logout");
                    if !target.starts_with('/') {
                        let span = logout.get("path").and_then(Node::span).unwrap_or_default();
                        self.push(SfRoute {
                            name: target.to_string(),
                            path: None,
                            file: path.clone(),
                            span,
                        });
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    const CONTROLLER: &str = r#"<?php
namespace App\Controller;

use Symfony\Component\Routing\Attribute\Route;

#[Route('/blog', name: 'blog_')]
class BlogController
{
    #[Route('/', name: 'index', methods: ['GET'])]
    public function index() {}

    #[Route('/{slug}', 'show')]
    public function show() {}

    #[Route('/unnamed')]
    public function unnamed() {}
}

#[Route('/health', name: 'health')]
class HealthController { public function __invoke() {} }
"#;

    fn routes(extra: &[(&str, &str)]) -> std::sync::Arc<SfRoutes> {
        let mut files = vec![
            ("src/Controller/BlogController.php", CONTROLLER),
            (
                "vendor/symfony/Route.php",
                "<?php namespace Symfony\\Component\\Routing\\Attribute; #[\\Attribute] class Route {}",
            ),
        ];
        files.extend_from_slice(extra);
        project(&files).section::<SfRoutes>()
    }

    #[test]
    fn reads_attribute_routes_with_the_prefix_of_their_class() {
        let routes = routes(&[]);
        assert_eq!(
            routes
                .find("blog_index")
                .and_then(|route| route.path.clone())
                .as_deref(),
            Some("/blog/")
        );
        assert_eq!(
            routes.find("blog_show").and_then(|route| route.path.clone()).as_deref(),
            Some("/blog/{slug}")
        );
        assert!(routes.find("health").is_some());
        assert!(
            routes.find("blog_app_blog_unnamed").is_some(),
            "{:?}",
            routes.routes.iter().map(|route| &route.name).collect::<Vec<_>>()
        );
        assert!(!routes.incomplete);
        assert!(routes.is_missing("nope"));
    }

    #[test]
    fn a_route_without_a_name_gets_the_one_symfony_makes_up() {
        assert_eq!(
            default_name("App\\Controller\\BlogController", "index"),
            "app_blog_index"
        );
        assert_eq!(
            default_name("App\\Controller\\Admin\\UserController", "editAction"),
            "app_admin_user_edit"
        );
        assert_eq!(
            default_name("Acme\\DemoBundle\\Controller\\PostController", "show"),
            "acme_demo_post_show"
        );
    }

    #[test]
    fn reads_the_routes_of_the_configuration_and_the_files_it_imports() {
        let routes = routes(&[
            (
                "config/routes.yaml",
                "controllers:\n    resource: routing.controllers\nhome:\n    path: /\n    controller: App\\Controller\\Home\nlegacy:\n    resource: '../src/Legacy/'\n    type: attribute\n_errors:\n    resource: '@FrameworkBundle/Resources/config/routing/errors.php'\n    prefix: /_error\n",
            ),
            (
                "config/bundles.php",
                "<?php return [\n    Symfony\\Bundle\\FrameworkBundle\\FrameworkBundle::class => ['all' => true],\n];",
            ),
            (
                "vendor/symfony/FrameworkBundle.php",
                "<?php namespace Symfony\\Bundle\\FrameworkBundle; class FrameworkBundle {}",
            ),
            (
                "vendor/symfony/Resources/config/routing/errors.php",
                "<?php return static function ($routes) { $routes->add('_preview_error', '/{code}'); };",
            ),
        ]);
        assert!(routes.find("home").is_some());
        assert!(routes.find("_preview_error").is_some());
        assert!(!routes.incomplete, "everything was followed");
    }

    #[test]
    fn an_import_that_cannot_be_followed_makes_the_list_incomplete() {
        let routes = routes(&[("config/routes.yaml", "api:\n    resource: .\n    type: api_platform\n")]);
        assert!(routes.incomplete);
        assert!(!routes.is_missing("nope"));
    }

    #[test]
    fn the_logout_route_is_named_by_the_firewall() {
        let routes = routes(&[
            (
                "config/routes/security.yaml",
                "_security_logout:\n    resource: security.route_loader.logout\n    type: service\n",
            ),
            (
                "config/packages/security.yaml",
                "security:\n    firewalls:\n        main:\n            logout:\n                path: app_logout\n",
            ),
        ]);
        assert!(routes.find("app_logout").is_some());
        assert!(!routes.incomplete);
    }
}
