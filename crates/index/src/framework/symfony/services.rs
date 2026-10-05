//! The service container of a Symfony project: the ids and classes of `services.yaml` and the
//! PHP configuration, and the parameters.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::{config_files, is_config, yaml_of};
use crate::framework::Section;
use crate::framework::source::{Literal, literal_of, resolver_for};
use crate::framework::yaml::Node;
use crate::index::Index;
use crate::model::Span;
use crate::test_facts::argument_expressions;
use crate::types::Name;

const KERNEL_TRAIT: &str = "Symfony\\Component\\DependencyInjection\\Kernel\\KernelTrait";

#[derive(Clone, Debug, PartialEq)]
pub struct ServiceDecl {
    pub id: String,
    pub class: Option<Name>,
    /// The id this one is another name for.
    pub alias_of: Option<String>,
    pub path: PathBuf,
    /// The id as written.
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParameterDecl {
    pub name: String,
    pub value: Option<String>,
    pub path: PathBuf,
    pub span: Span,
}

/// `App\: { resource: '../src/' }`: every class of a folder is a service with its class as the id.
#[derive(Clone, Debug, PartialEq)]
pub struct Resource {
    pub namespace: String,
    pub dir: PathBuf,
    pub exclude: Vec<PathBuf>,
}

#[derive(Default)]
pub struct Services {
    pub services: Vec<ServiceDecl>,
    pub parameters: Vec<ParameterDecl>,
    pub resources: Vec<Resource>,
    by_id: HashMap<String, usize>,
}

impl Services {
    pub fn find(&self, id: &str) -> Option<&ServiceDecl> {
        self.by_id.get(id).map(|at| &self.services[*at])
    }

    pub fn parameter(&self, name: &str) -> Option<&ParameterDecl> {
        self.parameters.iter().find(|parameter| parameter.name == name)
    }

    /// The class a service id stands for, following aliases. A class a resource covers is a service
    /// under its own name.
    pub fn class_of(&self, index: &Index, id: &str) -> Option<Name> {
        let mut current = id.trim_start_matches('\\').to_string();
        for _ in 0..8 {
            match self.find(&current) {
                Some(decl) => match (&decl.alias_of, &decl.class) {
                    (Some(target), _) => current = target.trim_start_matches('\\').to_string(),
                    (None, Some(class)) => return Some(class.clone()),
                    (None, None) => return index.class(&current).map(|class| class.decl.name.clone()),
                },
                None => return self.covers(index, &current).then_some(current),
            }
        }
        None
    }

    /// Whether a resource makes the class a service.
    pub fn covers(&self, index: &Index, class: &str) -> bool {
        let Some(found) = index.class(class) else {
            return false;
        };
        self.resources.iter().any(|resource| {
            class.starts_with(&resource.namespace)
                && found.file.path.starts_with(&resource.dir)
                && !resource
                    .exclude
                    .iter()
                    .any(|excluded| found.file.path.starts_with(excluded))
        })
    }
}

impl Section for Services {
    fn build(index: &Index) -> Self {
        let mut services = Services::default();
        for path in config_files(index, &["yaml", "yml"]) {
            if let Some((_, root)) = yaml_of(index, &path) {
                services.read_yaml(&path, &root);
            }
        }
        for path in config_files(index, &["php"]) {
            if let Some(text) = index.read_text(&path) {
                if text.contains("services()") || text.contains("parameters()") || text.contains("setParameter") {
                    services.read_php(&path, &parse(&text).syntax());
                }
            }
        }
        services.read_kernel_parameters(index);
        services
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        is_config(root, path)
    }
}

impl Services {
    fn push_service(&mut self, decl: ServiceDecl) {
        if self.by_id.contains_key(&decl.id) {
            return;
        }
        self.by_id.insert(decl.id.clone(), self.services.len());
        self.services.push(decl);
    }

    fn push_parameter(&mut self, parameter: ParameterDecl) {
        if self.parameter(&parameter.name).is_none() {
            self.parameters.push(parameter);
        }
    }

    fn read_yaml(&mut self, path: &Path, root: &Node) {
        let mut documents = vec![root];
        for (name, _, value) in root.entries() {
            if name.starts_with("when@") {
                documents.push(value);
            }
        }
        for document in documents {
            if let Some(parameters) = document.get("parameters") {
                for (name, span, value) in parameters.entries() {
                    self.push_parameter(ParameterDecl {
                        name: name.to_string(),
                        value: value.as_str().map(str::to_string),
                        path: path.to_path_buf(),
                        span,
                    });
                }
            }
            if let Some(services) = document.get("services") {
                for (id, span, value) in services.entries() {
                    self.read_service(path, id, span, value);
                }
            }
        }
    }

    fn read_service(&mut self, path: &Path, id: &str, span: Span, value: &Node) {
        if matches!(id, "_defaults" | "_instanceof") {
            return;
        }
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(resource) = value
            .get("resource")
            .and_then(Node::as_str)
            .filter(|_| id.ends_with('\\'))
        {
            let exclude = match value.get("exclude") {
                Some(Node::Seq(items)) => items.iter().filter_map(Node::as_str).map(str::to_string).collect(),
                Some(single) => single.as_str().map(str::to_string).into_iter().collect(),
                None => Vec::new(),
            };
            self.resources.push(Resource {
                namespace: id.to_string(),
                dir: normalize(&dir.join(strip_glob(resource))),
                exclude: exclude
                    .iter()
                    .map(|pattern| normalize(&dir.join(strip_glob(pattern))))
                    .collect(),
            });
            return;
        }
        let alias_of = match value {
            Node::Scalar { value, .. } => value.strip_prefix('@').map(str::to_string),
            other => other
                .get("alias")
                .and_then(Node::as_str)
                .map(|alias| alias.trim_start_matches('@').to_string()),
        };
        let class = value
            .get("class")
            .and_then(Node::as_str)
            .map(|class| class.trim_start_matches('\\').to_string())
            .or_else(|| id.contains('\\').then(|| id.trim_start_matches('\\').to_string()))
            .filter(|_| alias_of.is_none());
        self.push_service(ServiceDecl {
            id: id.trim_start_matches('\\').to_string(),
            class,
            alias_of,
            path: path.to_path_buf(),
            span,
        });
    }

    /// `$services->set('id', Foo::class)`, `->alias('id', Foo::class)` and `->load('App\\', '../src')`.
    fn read_php(&mut self, path: &Path, tree: &SyntaxNode) {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
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
            let receiver = callee
                .children()
                .next()
                .map(|node| node.text().to_string())
                .unwrap_or_default();
            let args = argument_expressions(&call);
            let text = |position: usize| match args.get(position).and_then(literal_of) {
                Some(Literal::Text(text, span)) => Some((text, span)),
                _ => None,
            };
            let class = |position: usize| match args.get(position).and_then(literal_of) {
                Some(Literal::Class(class)) => Some(class),
                _ => None,
            };
            match method.text().to_string().as_str() {
                "set" | "setParameter" if receiver.contains("parameters") || method.text() == "setParameter" => {
                    if let Some((name, span)) = text(0) {
                        self.push_parameter(ParameterDecl {
                            name,
                            value: text(1).map(|(text, _)| text),
                            path: path.to_path_buf(),
                            span,
                        });
                    }
                }
                "set" | "register" if receiver.contains("services") || receiver.contains("container") => {
                    if let Some(id) = class(0)
                        .map(|class| (class.clone(), Span::default()))
                        .or_else(|| text(0))
                    {
                        let concrete = class(1)
                            .or_else(|| class(0))
                            .or_else(|| text(1).map(|(text, _)| text).filter(|text| text.contains('\\')));
                        let _ = resolver_for;
                        self.push_service(ServiceDecl {
                            id: id.0,
                            class: concrete,
                            alias_of: None,
                            path: path.to_path_buf(),
                            span: id.1,
                        });
                    }
                }
                "alias" | "setAlias" => {
                    let target = class(1).or_else(|| text(1).map(|(text, _)| text));
                    if let (Some((id, span)), Some(target)) =
                        (class(0).map(|c| (c, Span::default())).or_else(|| text(0)), target)
                    {
                        self.push_service(ServiceDecl {
                            id,
                            class: None,
                            alias_of: Some(target),
                            path: path.to_path_buf(),
                            span,
                        });
                    }
                }
                "load" if receiver.contains("services") => {
                    if let (Some((namespace, _)), Some((resource, _))) = (text(0), text(1)) {
                        self.resources.push(Resource {
                            namespace,
                            dir: normalize(&dir.join(strip_glob(&resource))),
                            exclude: Vec::new(),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    /// The parameters the kernel defines, from the array its trait returns.
    fn read_kernel_parameters(&mut self, index: &Index) {
        let Some(class) = index.class(KERNEL_TRAIT) else {
            return;
        };
        let Some(method) = class.decl.method("getKernelParameters") else {
            return;
        };
        let Some(tree) = crate::framework::source::tree_of(index, &class.file.path) else {
            return;
        };
        let Some(declaration) = crate::framework::source::method_at(&tree, method.name_span.start) else {
            return;
        };
        for item in declaration.descendants().filter(|node| node.kind() == ARRAY_ITEM) {
            let parts: Vec<SyntaxNode> = item.children().collect();
            if let [key, _] = parts.as_slice() {
                if let Some(Literal::Text(name, span)) = literal_of(key) {
                    self.push_parameter(ParameterDecl {
                        name,
                        value: None,
                        path: class.file.path.clone(),
                        span,
                    });
                }
            }
        }
    }
}

/// `../src/` and `../src/{Entity,Kernel.php}` as the folder before the first pattern.
fn strip_glob(path: &str) -> String {
    let cut = path.find(['*', '{', '?', '[']).unwrap_or(path.len());
    let kept = &path[..cut];
    kept.rsplit_once('/')
        .filter(|_| cut < path.len())
        .map_or(kept, |(head, _)| head)
        .to_string()
}

/// Resolves `..` and `.`.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    const SERVICES: &str = "parameters:\n    app.admin: 'a@b.c'\nservices:\n    _defaults:\n        autowire: true\n    App\\:\n        resource: '../src/'\n        exclude: ['../src/Entity/', '../src/Kernel.php']\n    app.mailer:\n        class: App\\Service\\Mailer\n    App\\Service\\Plain: ~\n    app.alias: '@app.mailer'\n    App\\Contract\\Notifier:\n        alias: App\\Service\\Plain\nwhen@dev:\n    services:\n        app.dev:\n            class: App\\Dev\n";

    fn services() -> (crate::index::Index, std::sync::Arc<Services>) {
        let index = project(&[
            ("config/services.yaml", SERVICES),
            (
                "src/Service/Mailer.php",
                "<?php namespace App\\Service; class Mailer {}",
            ),
            ("src/Service/Plain.php", "<?php namespace App\\Service; class Plain {}"),
            ("src/Service/Other.php", "<?php namespace App\\Service; class Other {}"),
            ("src/Entity/User.php", "<?php namespace App\\Entity; class User {}"),
            (
                "config/packages/app.php",
                "<?php return function ($container) { $services = $container->services(); $services->set('app.php', \\App\\Service\\Other::class); $services->alias('app.php2', \\App\\Service\\Other::class); $container->parameters()->set('app.limit', 10); };",
            ),
        ]);
        let found = index.section::<Services>();
        (index, found)
    }

    #[test]
    fn reads_the_ids_aliases_and_parameters() {
        let (index, services) = services();
        assert_eq!(
            services.class_of(&index, "app.mailer").as_deref(),
            Some("App\\Service\\Mailer")
        );
        assert_eq!(
            services.class_of(&index, "app.alias").as_deref(),
            Some("App\\Service\\Mailer")
        );
        assert_eq!(
            services.class_of(&index, "App\\Contract\\Notifier").as_deref(),
            Some("App\\Service\\Plain")
        );
        assert_eq!(
            services.class_of(&index, "App\\Service\\Plain").as_deref(),
            Some("App\\Service\\Plain")
        );
        assert_eq!(services.class_of(&index, "app.dev").as_deref(), Some("App\\Dev"));
        assert_eq!(
            services.class_of(&index, "app.php").as_deref(),
            Some("App\\Service\\Other")
        );
        assert_eq!(
            services.class_of(&index, "app.php2").as_deref(),
            Some("App\\Service\\Other")
        );
        assert_eq!(services.class_of(&index, "nope"), None);
        assert!(services.parameter("app.admin").is_some());
        assert!(services.parameter("app.limit").is_some());
        let decl = services.find("app.mailer").expect("a service");
        assert_eq!(
            &SERVICES[decl.span.start as usize..decl.span.end as usize],
            "app.mailer"
        );
    }

    #[test]
    fn a_resource_covers_the_classes_it_does_not_exclude() {
        let (index, services) = services();
        assert_eq!(
            services.class_of(&index, "App\\Service\\Other").as_deref(),
            Some("App\\Service\\Other")
        );
        assert_eq!(services.class_of(&index, "App\\Entity\\User"), None, "excluded");
    }
}
