//! Livewire: the components of a project by the name a template writes them under
//! (`<livewire:edit-reply>`, `@livewire('likes.thread')`), each with its class and the view it
//! renders. A component's name is its class below the configured namespace (`App\Livewire` unless
//! `livewire.class_namespace` says otherwise) in kebab case, folders joined with dots, or the name
//! `Livewire::component()` registers it under.

use std::path::{Path, PathBuf};

use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::Section;
use super::config::ConfigKeys;
use super::layouts::kebab;
use super::source::{Literal, literal_of, method_at, resolver_for, returned_expression, tree_of};
use crate::index::{Index, Origin};
use crate::model::Span;
use crate::test_facts::argument_expressions;
use crate::types::{Name, Type};

pub const COMPONENT: &str = "Livewire\\Component";
const DEFAULT_NAMESPACES: [&str; 2] = ["App\\Livewire", "App\\Http\\Livewire"];

#[derive(Clone, Debug, PartialEq)]
pub struct LivewireComponent {
    pub name: String,
    pub class: Name,
    /// The view it renders, by name: the one `render()` returns, or `livewire.{name}` without one.
    pub view: Option<String>,
    pub path: PathBuf,
    pub name_span: Span,
}

#[derive(Default)]
pub struct Livewire {
    pub components: Vec<LivewireComponent>,
}

impl Livewire {
    pub fn find(&self, name: &str) -> Option<&LivewireComponent> {
        self.components.iter().find(|component| component.name == name)
    }

    /// The component a view belongs to, when exactly one renders it.
    pub fn of_view(&self, view: &str) -> Option<&LivewireComponent> {
        let view = view.replace('/', ".");
        let mut found = self
            .components
            .iter()
            .filter(|component| component.view.as_deref() == Some(view.as_str()));
        let first = found.next()?;
        found.next().is_none().then_some(first)
    }
}

impl Section for Livewire {
    fn build(index: &Index) -> Self {
        let mut livewire = Livewire::default();
        if index.class(COMPONENT).is_none() {
            return livewire;
        }
        let configured = index
            .section::<ConfigKeys>()
            .find("livewire.class_namespace")
            .and_then(|entry| entry.value.clone())
            .map(|value| value.trim_matches(['\'', '"']).replace("\\\\", "\\"))
            .filter(|value| !value.is_empty());
        let namespaces: Vec<String> = match configured {
            Some(namespace) => vec![namespace.trim_matches('\\').to_string()],
            None => DEFAULT_NAMESPACES
                .iter()
                .map(|namespace| namespace.to_string())
                .collect(),
        };
        for class in index.all_subtypes(COMPONENT) {
            if class.file.origin != Origin::Project || class.decl.is_abstract {
                continue;
            }
            let Some(name) = namespaces
                .iter()
                .find_map(|namespace| name_below(namespace, &class.decl.name))
            else {
                continue;
            };
            let view = view_of(index, &class.decl.name).unwrap_or_else(|| Some(format!("livewire.{name}")));
            livewire.components.push(LivewireComponent {
                name,
                class: class.decl.name.clone(),
                view,
                path: class.file.path.clone(),
                name_span: class.decl.name_span,
            });
        }
        for provider in index.all_subtypes("Illuminate\\Support\\ServiceProvider") {
            if provider.file.origin != Origin::Project {
                continue;
            }
            let Some(tree) = tree_of(index, &provider.file.path) else {
                continue;
            };
            for call in tree.descendants().filter(|node| node.kind() == CALL_EXPR) {
                if let Some((name, class)) = registered(&call) {
                    let Some(found) = index.class(&class) else {
                        continue;
                    };
                    let view = view_of(index, &found.decl.name).unwrap_or(None);
                    livewire.components.push(LivewireComponent {
                        name,
                        class: found.decl.name.clone(),
                        view,
                        path: found.file.path.clone(),
                        name_span: found.decl.name_span,
                    });
                }
            }
        }
        livewire
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

/// `App\Livewire\Forum\EditReply` below `App\Livewire` is `forum.edit-reply`.
fn name_below(namespace: &str, class: &str) -> Option<String> {
    let rest = class.strip_prefix(namespace)?.strip_prefix('\\')?;
    Some(rest.split('\\').map(kebab).collect::<Vec<_>>().join("."))
}

/// The view a component's `render()` returns by name. `None` when the component has no `render()` of
/// its own or above it, `Some(None)` when it has one that names no view plainly.
fn view_of(index: &Index, class: &str) -> Option<Option<String>> {
    let found = index.find_method(&Type::class(class), "render")?;
    if found.class.decl.name.eq_ignore_ascii_case(COMPONENT) {
        return None;
    }
    let tree = tree_of(index, &found.class.file.path)?;
    let method = method_at(&tree, found.member.name_span.start)?;
    let returned = returned_expression(&method);
    Some(returned.as_ref().and_then(view_name))
}

/// `view('livewire.edit-reply')` and `view('livewire.edit-reply', [...])`.
fn view_name(expression: &SyntaxNode) -> Option<String> {
    if expression.kind() != CALL_EXPR {
        return None;
    }
    let callee = expression.children().next()?;
    if callee.kind() != NAME
        || !callee
            .text()
            .to_string()
            .trim_start_matches('\\')
            .eq_ignore_ascii_case("view")
    {
        return None;
    }
    match argument_expressions(expression).first().and_then(literal_of)? {
        Literal::Text(name, _) => Some(name),
        Literal::Class(_) => None,
    }
}

/// `Livewire::component('name', Component::class)`.
fn registered(call: &SyntaxNode) -> Option<(String, Name)> {
    let callee = call
        .children()
        .next()
        .filter(|callee| callee.kind() == SCOPED_ACCESS_EXPR)?;
    let mut names = callee.children().filter(|child| child.kind() == NAME);
    let (class, method) = (names.next()?, names.next()?);
    if !method.text().to_string().eq_ignore_ascii_case("component") {
        return None;
    }
    let facade = resolver_for(&callee).resolve_class(&class.text().to_string());
    if !matches!(facade.as_str(), "Livewire\\Livewire" | "Livewire") {
        return None;
    }
    let args = argument_expressions(call);
    let Some(Literal::Text(name, _)) = args.first().and_then(literal_of) else {
        return None;
    };
    let Some(Literal::Class(class)) = args.get(1).and_then(literal_of) else {
        return None;
    };
    Some((name, class))
}
