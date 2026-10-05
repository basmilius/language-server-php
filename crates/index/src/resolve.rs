//! Name resolution: what a name written in a file refers to, given the namespace and the `use`
//! statements above it.

use std::collections::HashMap;

use crate::types::Name;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseKind {
    Class,
    Function,
    Constant,
}

/// The namespace and imports in effect at a point of a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameResolver {
    pub namespace: String,
    classes: HashMap<String, Name>,
    functions: HashMap<String, Name>,
    constants: HashMap<String, Name>,
}

impl NameResolver {
    pub fn new(namespace: &str) -> NameResolver {
        NameResolver {
            namespace: namespace.trim_matches('\\').to_string(),
            ..NameResolver::default()
        }
    }

    pub fn add_use(&mut self, kind: UseKind, name: &str, alias: Option<&str>) {
        let name = name.trim_start_matches('\\').to_string();
        let alias = alias
            .map(str::to_string)
            .unwrap_or_else(|| name.rsplit('\\').next().unwrap_or(&name).to_string());
        match kind {
            UseKind::Class => {
                self.classes.insert(alias.to_ascii_lowercase(), name);
            }
            UseKind::Function => {
                self.functions.insert(alias.to_ascii_lowercase(), name);
            }
            UseKind::Constant => {
                self.constants.insert(alias, name);
            }
        }
    }

    /// The name a declaration in this namespace gets.
    pub fn qualify(&self, short: &str) -> Name {
        if self.namespace.is_empty() {
            short.to_string()
        } else {
            format!("{}\\{short}", self.namespace)
        }
    }

    /// The class a written name refers to. `self`, `static` and `parent` are the caller's business.
    pub fn resolve_class(&self, raw: &str) -> Name {
        if let Some(rest) = raw.strip_prefix('\\') {
            return rest.to_string();
        }
        if let Some(rest) = strip_namespace_keyword(raw) {
            return self.qualify(rest);
        }
        let (first, rest) = match raw.split_once('\\') {
            Some((first, rest)) => (first, Some(rest)),
            None => (raw, None),
        };
        if let Some(target) = self.classes.get(&first.to_ascii_lowercase()) {
            return match rest {
                Some(rest) => format!("{target}\\{rest}"),
                None => target.clone(),
            };
        }
        self.qualify(raw)
    }

    /// The names a written function name may refer to, in the order PHP tries them.
    pub fn function_candidates(&self, raw: &str) -> Vec<Name> {
        self.value_candidates(raw, &self.functions, true)
    }

    pub fn constant_candidates(&self, raw: &str) -> Vec<Name> {
        self.value_candidates(raw, &self.constants, false)
    }

    fn value_candidates(&self, raw: &str, imports: &HashMap<String, Name>, fold: bool) -> Vec<Name> {
        if let Some(rest) = raw.strip_prefix('\\') {
            return vec![rest.to_string()];
        }
        if let Some(rest) = strip_namespace_keyword(raw) {
            return vec![self.qualify(rest)];
        }
        if let Some((first, rest)) = raw.split_once('\\') {
            let key = first.to_ascii_lowercase();
            return match self.classes.get(&key) {
                Some(target) => vec![format!("{target}\\{rest}")],
                None => vec![self.qualify(raw)],
            };
        }
        let key = if fold {
            raw.to_ascii_lowercase()
        } else {
            raw.to_string()
        };
        if let Some(target) = imports.get(&key) {
            return vec![target.clone()];
        }
        let mut out = Vec::new();
        if !self.namespace.is_empty() {
            out.push(self.qualify(raw));
        }
        out.push(raw.to_string());
        out
    }

    /// The alias a class is imported under, if it is.
    pub fn alias_of_class(&self, name: &str) -> Option<&str> {
        self.classes
            .iter()
            .find(|(_, target)| target.eq_ignore_ascii_case(name))
            .map(|(alias, _)| alias.as_str())
    }

    /// Every import of a kind as `(alias, target)`.
    pub fn imports(&self, kind: UseKind) -> impl Iterator<Item = (&str, &str)> {
        let map = match kind {
            UseKind::Class => &self.classes,
            UseKind::Function => &self.functions,
            UseKind::Constant => &self.constants,
        };
        map.iter().map(|(alias, target)| (alias.as_str(), target.as_str()))
    }
}

fn strip_namespace_keyword(raw: &str) -> Option<&str> {
    let (head, rest) = raw.split_once('\\')?;
    head.eq_ignore_ascii_case("namespace").then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> NameResolver {
        let mut resolver = NameResolver::new("App\\Http");
        resolver.add_use(UseKind::Class, "Illuminate\\Support\\Collection", None);
        resolver.add_use(UseKind::Class, "Foo\\Bar", Some("Baz"));
        resolver.add_use(UseKind::Function, "Foo\\helper", None);
        resolver.add_use(UseKind::Constant, "Foo\\LIMIT", None);
        resolver
    }

    #[test]
    fn resolves_class_names() {
        let resolver = resolver();
        assert_eq!(resolver.resolve_class("Collection"), "Illuminate\\Support\\Collection");
        assert_eq!(resolver.resolve_class("collection"), "Illuminate\\Support\\Collection");
        assert_eq!(resolver.resolve_class("Baz\\Deep"), "Foo\\Bar\\Deep");
        assert_eq!(resolver.resolve_class("\\Exception"), "Exception");
        assert_eq!(resolver.resolve_class("Exception"), "App\\Http\\Exception");
        assert_eq!(resolver.resolve_class("namespace\\Sub\\X"), "App\\Http\\Sub\\X");
        assert_eq!(resolver.resolve_class("Sub\\X"), "App\\Http\\Sub\\X");
    }

    #[test]
    fn functions_and_constants_fall_back_to_the_global_namespace() {
        let resolver = resolver();
        assert_eq!(resolver.function_candidates("helper"), vec!["Foo\\helper"]);
        assert_eq!(
            resolver.function_candidates("strlen"),
            vec!["App\\Http\\strlen", "strlen"]
        );
        assert_eq!(resolver.function_candidates("\\strlen"), vec!["strlen"]);
        assert_eq!(resolver.constant_candidates("LIMIT"), vec!["Foo\\LIMIT"]);
        assert_eq!(resolver.constant_candidates("limit"), vec!["App\\Http\\limit", "limit"]);
        assert_eq!(resolver.function_candidates("Baz\\run"), vec!["Foo\\Bar\\run"]);
    }

    #[test]
    fn the_global_namespace_has_no_prefix() {
        let resolver = NameResolver::new("");
        assert_eq!(resolver.resolve_class("Foo"), "Foo");
        assert_eq!(resolver.function_candidates("f"), vec!["f"]);
    }
}
