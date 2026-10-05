//! The service container: which class a name the framework registers itself stands for.

use std::collections::HashMap;
use std::path::Path;

use php_syntax::SyntaxKind::*;

use super::Section;
use super::source::{Literal, array_items, literal_of, resolver_for, returned_expression, tree_of};
use crate::index::{Index, Origin};
use crate::test_facts::argument_expressions;
use crate::types::{Name, Type};
use php_syntax::SyntaxNode;

pub const APPLICATION: &str = "Illuminate\\Foundation\\Application";

/// The aliases the application registers for the framework's own services (`cache`, `db`, `config`),
/// read from the array `registerCoreContainerAliases()` loops over.
#[derive(Default)]
pub struct CoreAliases {
    classes: HashMap<String, Name>,
}

impl CoreAliases {
    pub fn class_of(&self, alias: &str) -> Option<Name> {
        self.classes.get(alias).cloned()
    }
}

impl Section for CoreAliases {
    fn build(index: &Index) -> Self {
        let mut aliases = CoreAliases::default();
        let Some(application) = index.class(APPLICATION) else {
            return aliases;
        };
        let Some(method) = application.decl.method("registerCoreContainerAliases") else {
            return aliases;
        };
        let Some(tree) = tree_of(index, &application.file.path) else {
            return aliases;
        };
        let Some(declaration) = super::source::method_at(&tree, method.name_span.start) else {
            return aliases;
        };
        let Some(list) = declaration.descendants().find(|node| node.kind() == ARRAY_EXPR) else {
            return aliases;
        };
        for (key, value) in array_items(&list).unwrap_or_default() {
            let Some(Literal::Text(alias, _)) = key.as_ref().and_then(literal_of) else {
                continue;
            };
            let Some(first) = array_items(&value).and_then(|items| items.into_iter().next()) else {
                continue;
            };
            let class = match literal_of(&first.1) {
                Some(Literal::Class(name)) => Some(name),
                _ => self_class(&first.1, application.decl.name.as_str()),
            };
            if let Some(class) = class {
                aliases.classes.insert(alias, class);
            }
        }
        aliases
    }

    fn depends_on(_: &Path, _: &Path) -> bool {
        false
    }
}

/// `self::class` inside the application stands for the application.
fn self_class(node: &php_syntax::SyntaxNode, application: &str) -> Option<Name> {
    let text = node.text().to_string();
    (text.replace(' ', "") == "self::class").then(|| application.to_string())
}

/// The class a name the container knows stands for: a binding a service provider of the project makes
/// with a literal name, else one of the framework's own aliases.
pub fn class_of(index: &Index, name: &str) -> Option<Name> {
    let frameworks = index.frameworks();
    if frameworks.symfony {
        if let Some(class) = index
            .section::<super::symfony::services::Services>()
            .class_of(index, name)
        {
            return Some(class);
        }
    }
    if !(frameworks.laravel || frameworks.facades) {
        return None;
    }
    index
        .section::<Bindings>()
        .class_of(name)
        .or_else(|| index.section::<CoreAliases>().class_of(name))
}

pub fn type_of(index: &Index, name: &str) -> Option<Type> {
    class_of(index, name).map(Type::class)
}

const SERVICE_PROVIDER: &str = "Illuminate\\Support\\ServiceProvider";
const BINDING_METHODS: [&str; 7] = [
    "bind",
    "singleton",
    "scoped",
    "bindif",
    "singletonif",
    "scopedif",
    "instance",
];

/// What the service providers of the project register under a name written out: `$this->app->bind('mail',
/// fn () => new Mailer)` and `$this->app->singleton(Gateway::class, StripeGateway::class)`.
#[derive(Default)]
pub struct Bindings {
    by_name: HashMap<String, Name>,
}

impl Bindings {
    pub fn class_of(&self, name: &str) -> Option<Name> {
        self.by_name.get(name.trim_start_matches('\\')).cloned()
    }

    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.by_name.keys()
    }
}

impl Section for Bindings {
    fn build(index: &Index) -> Self {
        let mut bindings = Bindings::default();
        let mut paths = Vec::new();
        for provider in index.all_subtypes(SERVICE_PROVIDER) {
            if provider.file.origin == Origin::Project && !paths.contains(&provider.file.path) {
                paths.push(provider.file.path.clone());
            }
        }
        for path in paths {
            if let Some(tree) = tree_of(index, &path) {
                for node in tree.descendants().filter(|node| node.kind() == CALL_EXPR) {
                    bindings.read_call(&node);
                }
            }
        }
        bindings
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

impl Bindings {
    fn read_call(&mut self, call: &SyntaxNode) {
        let Some(callee) = call
            .children()
            .next()
            .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
        else {
            return;
        };
        let Some(method) = callee.children().find(|child| child.kind() == NAME) else {
            return;
        };
        let method = method.text().to_string().to_ascii_lowercase();
        let receiver = callee
            .children()
            .next()
            .map(|node| node.text().to_string())
            .unwrap_or_default();
        let receiver = receiver.replace(char::is_whitespace, "");
        if !matches!(receiver.as_str(), "$this->app" | "$app" | "app()") {
            return;
        }
        let args = argument_expressions(call);
        if method == "alias" {
            if let (Some(Literal::Class(class)), Some(Literal::Text(alias, _))) =
                (args.first().and_then(literal_of), args.get(1).and_then(literal_of))
            {
                self.by_name.insert(alias, class);
            }
            return;
        }
        if !BINDING_METHODS.contains(&method.as_str()) {
            return;
        }
        let abstract_name = match args.first().and_then(literal_of) {
            Some(Literal::Class(class)) => class,
            Some(Literal::Text(text, _)) => text.trim_start_matches('\\').to_string(),
            None => return,
        };
        let concrete = match args.get(1) {
            None => Some(abstract_name.clone()).filter(|name| name.contains('\\')),
            Some(node) => concrete_of(node),
        };
        if let Some(concrete) = concrete {
            self.by_name.insert(abstract_name, concrete);
        }
    }
}

/// The class a binding's second argument stands for: a class name, `new Foo` or a closure that
/// returns one.
fn concrete_of(node: &SyntaxNode) -> Option<Name> {
    match node.kind() {
        NEW_EXPR => {
            let class = node.children().find(|child| child.kind() == NAME)?;
            Some(resolver_for(node).resolve_class(&class.text().to_string())).filter(|name| name != "static")
        }
        CLOSURE_EXPR | ARROW_FUNCTION_EXPR => {
            let resolver = resolver_for(node);
            let (callable, _) = crate::extract::callable_at(node, &resolver, None);
            if let Some(Type::Class { name, .. }) = callable.ret {
                return Some(name);
            }
            let returned = if node.kind() == ARROW_FUNCTION_EXPR {
                node.children().last()
            } else {
                returned_expression(node)
            }?;
            concrete_of(&returned)
        }
        CALL_EXPR => {
            let callee = node.children().next()?;
            let method = callee.children().find(|child| child.kind() == NAME)?;
            if callee.kind() != PROPERTY_FETCH_EXPR
                || !matches!(method.text().to_string().as_str(), "make" | "get" | "makeWith")
            {
                return None;
            }
            match argument_expressions(node).first().and_then(literal_of)? {
                Literal::Class(class) => Some(class),
                Literal::Text(..) => None,
            }
        }
        _ => match literal_of(node)? {
            Literal::Class(class) => Some(class),
            Literal::Text(text, _) if text.contains('\\') => Some(text.trim_start_matches('\\').to_string()),
            Literal::Text(..) => None,
        },
    }
}
