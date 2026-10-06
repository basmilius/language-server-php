//! The Raxos router's tree of controllers: `#[Controller(prefix:)]`, the `#[Child]` controllers below
//! it and the routes of its methods, so a route has the whole path it answers to and a controller
//! knows the controllers whose values reach it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::super::source::class_in_attribute;
use crate::framework::Section;
use crate::index::{Index, Origin};
use crate::model::{Attribute, Span};
use crate::types::Name;

const ATTRIBUTE: &str = "Raxos\\Router\\Attribute\\";
pub const CONTROLLER: &str = "Raxos\\Router\\Attribute\\Controller";
pub const CHILD: &str = "Raxos\\Router\\Attribute\\Child";
pub const MAP_MODEL_RELATION: &str = "Raxos\\Router\\Attribute\\MapModelRelation";
pub const VERBS: [&str; 8] = ["Get", "Post", "Put", "Patch", "Delete", "Head", "Options", "Any"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteInfo {
    pub method: String,
    /// The attribute's short name in upper case: `GET`, `ANY`.
    pub verb: String,
    pub path: String,
    pub name_span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerInfo {
    pub class: Name,
    pub path: PathBuf,
    pub prefix: String,
    pub children: Vec<Name>,
    pub routes: Vec<RouteInfo>,
}

/// The controllers of the project, by lowercase class name, with the controllers each is a child of.
#[derive(Default)]
pub struct Controllers {
    by_class: HashMap<String, ControllerInfo>,
    parents: HashMap<String, Vec<Name>>,
}

impl Controllers {
    pub fn get(&self, class: &str) -> Option<&ControllerInfo> {
        self.by_class.get(&class.trim_start_matches('\\').to_ascii_lowercase())
    }

    pub fn all(&self) -> impl Iterator<Item = &ControllerInfo> {
        self.by_class.values()
    }

    /// The controllers whose constructor values reach a controller's routes: itself, then the ones
    /// it is a child of, directly or further up.
    pub fn providers(&self, class: &str) -> Vec<Name> {
        let mut out = vec![class.to_string()];
        out.extend(self.above(class));
        out
    }

    /// The controllers a controller is a child of, directly or further up, each once.
    pub fn above(&self, class: &str) -> Vec<Name> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = vec![class.to_ascii_lowercase()];
        while let Some(current) = queue.pop() {
            for parent in self.parents.get(&current).into_iter().flatten() {
                if seen.insert(parent.to_ascii_lowercase()) {
                    queue.push(parent.to_ascii_lowercase());
                    out.push(parent.clone());
                }
            }
        }
        out
    }

    /// Every path a controller is mounted at: the prefixes of each chain of parents down to it. A
    /// controller no other one has as a child is mounted at its own prefix.
    pub fn mounts(&self, class: &str) -> Vec<String> {
        self.mounts_below(class, &mut HashSet::new())
    }

    fn mounts_below(&self, class: &str, visiting: &mut HashSet<String>) -> Vec<String> {
        let key = class.to_ascii_lowercase();
        let Some(info) = self.by_class.get(&key) else {
            return Vec::new();
        };
        if !visiting.insert(key.clone()) {
            return Vec::new();
        }
        let own = normalize_prefix(&info.prefix);
        let parents = self.parents.get(&key).cloned().unwrap_or_default();
        let mut out: Vec<String> = if parents.is_empty() {
            vec![own.trim_end_matches('/').to_string()]
        } else {
            parents
                .iter()
                .flat_map(|parent| self.mounts_below(parent, visiting))
                .map(|above| format!("{above}{own}").trim_end_matches('/').to_string())
                .collect()
        };
        visiting.remove(&key);
        out.dedup();
        out
    }

    /// The whole paths of a route, one per mount of its controller, as `GET /merchants/$merchant`.
    pub fn route_paths(&self, class: &str, route: &RouteInfo) -> Vec<String> {
        self.mounts(class)
            .into_iter()
            .map(|mount| {
                let path = format!("{mount}{}", normalize_path(&route.path));
                let path = if path.is_empty() { "/".to_string() } else { path };
                format!("{} {path}", route.verb)
            })
            .collect()
    }
}

impl Section for Controllers {
    fn build(index: &Index) -> Self {
        let mut found = Controllers::default();
        for class in index.class_names() {
            if class.file.origin != Origin::Project {
                continue;
            }
            let Some(loaded) = class.load() else {
                continue;
            };
            let decl = loaded.decl;
            let Some(controller) = decl
                .attributes
                .iter()
                .find(|attribute| attribute.name.eq_ignore_ascii_case(CONTROLLER))
            else {
                continue;
            };
            let prefix = string_argument(controller, "prefix").unwrap_or_else(|| "/".to_string());
            let children: Vec<Name> = decl
                .attributes
                .iter()
                .filter(|attribute| attribute.name.eq_ignore_ascii_case(CHILD))
                .filter_map(|attribute| attribute.args.first())
                .filter_map(|arg| class_in_attribute(index, loaded, &arg.value))
                .collect();
            let routes = decl
                .methods
                .iter()
                .flat_map(|method| {
                    method.attributes.iter().filter_map(|attribute| {
                        let verb = route_verb(&attribute.name)?;
                        Some(RouteInfo {
                            method: method.name.clone(),
                            verb: verb.to_ascii_uppercase(),
                            path: string_argument(attribute, "path").unwrap_or_else(|| "/".to_string()),
                            name_span: method.name_span,
                        })
                    })
                })
                .collect();
            for child in &children {
                found
                    .parents
                    .entry(child.to_ascii_lowercase())
                    .or_default()
                    .push(decl.name.clone());
            }
            found.by_class.insert(
                decl.name.to_ascii_lowercase(),
                ControllerInfo {
                    class: decl.name.clone(),
                    path: loaded.file.path.clone(),
                    prefix,
                    children,
                    routes,
                },
            );
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_project_php(root, path)
    }
}

/// The verb of a route attribute by its class: `Get` for `#[Get]`, `None` for any other attribute.
pub fn route_verb(attribute: &str) -> Option<&'static str> {
    let short = attribute.strip_prefix(ATTRIBUTE)?;
    VERBS.iter().copied().find(|verb| verb.eq_ignore_ascii_case(short))
}

/// The names of the parameters a path asks for: `deliveries/$deliveryId` asks for `deliveryId`,
/// with the offset of each `$` in the path.
pub fn path_parameters(path: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    for (at, _) in path.match_indices('$') {
        let rest = &path[at + 1..];
        let length = rest
            .char_indices()
            .find(|(position, character)| {
                !(character.is_ascii_alphanumeric() || *character == '_')
                    || *position == 0 && character.is_ascii_digit()
            })
            .map_or(rest.len(), |(position, _)| position);
        if length > 0 {
            out.push((at, &rest[..length]));
        }
    }
    out
}

/// A prefix as the mapper joins it: always one leading slash.
fn normalize_prefix(prefix: &str) -> String {
    format!("/{}", prefix.trim_start_matches('/'))
}

/// A route path as the router's `normalizePath` writes it: nothing for the root, else one leading
/// slash before a name or a parameter.
fn normalize_path(path: &str) -> String {
    if path.is_empty() || path == "/" {
        return String::new();
    }
    match path.chars().next() {
        Some(first) if first == '$' || first.is_ascii_alphanumeric() => format!("/{path}"),
        _ => path.to_string(),
    }
}

fn string_argument(attribute: &Attribute, name: &str) -> Option<String> {
    let value = attribute
        .args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(name))
        .or_else(|| attribute.args.first().filter(|arg| arg.name.is_none()))?;
    let text = value.value.trim();
    let quoted = text.len() >= 2
        && (text.starts_with('\'') && text.ends_with('\'') || text.starts_with('"') && text.ends_with('"'));
    quoted.then(|| text[1..text.len() - 1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::{RAXOS, project};

    fn with_controllers() -> Index {
        let mut files = RAXOS.to_vec();
        files.extend_from_slice(&[
            (
                "src/MerchantController.php",
                "<?php namespace App;\nuse Raxos\\Router\\Attribute\\{Child, Controller, Get};\n#[Controller(prefix: '/merchants/$merchant')]\n#[Child(MerchantEventController::class)]\nclass MerchantController {\n    public function __construct(public Merchant $merchant) {}\n    #[Get] public function get(): Merchant {}\n}",
            ),
            (
                "src/MerchantEventController.php",
                "<?php namespace App;\nuse Raxos\\Router\\Attribute\\{Controller, Get, Post};\n#[Controller(prefix: 'events/$event')]\nclass MerchantEventController {\n    #[Get('deliveries/$deliveryId')] public function delivery(int $deliveryId): Delivery {}\n    #[Post] public function update(): Event {}\n}",
            ),
        ]);
        project(&files)
    }

    #[test]
    fn a_route_has_the_whole_path_of_its_controllers() {
        let index = with_controllers();
        let controllers = index.section::<Controllers>();
        let info = controllers.get("App\\MerchantEventController").expect("a controller");
        let paths: Vec<String> = info
            .routes
            .iter()
            .flat_map(|route| controllers.route_paths(&info.class, route))
            .collect();
        assert_eq!(
            paths,
            [
                "GET /merchants/$merchant/events/$event/deliveries/$deliveryId",
                "POST /merchants/$merchant/events/$event"
            ]
        );
        assert_eq!(
            controllers.above("App\\MerchantEventController"),
            ["App\\MerchantController"]
        );
    }

    #[test]
    fn reads_the_parameters_of_a_path() {
        assert_eq!(
            path_parameters("/a/$event/b/$delivery_id.json"),
            [(3, "event"), (12, "delivery_id")]
        );
        assert_eq!(path_parameters("price/$5"), []);
    }
}
