//! The overlays: PHP files that say what the frameworks do at run time, read as data. The tags on
//! a declaration are the data, and the declarations themselves are never run.

use std::collections::HashMap;
use std::sync::OnceLock;

use php_syntax::parse;

use crate::extract::{ExtractOptions, extract};
use crate::index::Index;
use crate::model::{ClassDecl, Doc, FileSymbols, Method};

fn laravel() -> &'static FileSymbols {
    static PARSED: OnceLock<FileSymbols> = OnceLock::new();
    PARSED.get_or_init(|| {
        extract(
            &parse(include_str!("laravel_overlay.php")).syntax(),
            ExtractOptions::default(),
        )
    })
}

fn symfony() -> &'static FileSymbols {
    static PARSED: OnceLock<FileSymbols> = OnceLock::new();
    PARSED.get_or_init(|| {
        extract(
            &parse(include_str!("symfony_overlay.php")).syntax(),
            ExtractOptions::default(),
        )
    })
}

/// Which framework an overlay entry belongs to, which has to be in the project for it to count.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Laravel,
    Symfony,
}

/// What a function or method of the overlay says about its arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Marker {
    /// The argument at `position`, or the one with this name when it is written as a named argument,
    /// names something a project declares: `config`, `route`, `view`, `translation`, `env`, `ability`,
    /// `service`, `parameter`, `template` and the like.
    Key {
        kind: String,
        position: usize,
        name: Option<String>,
    },
    /// The argument at `position` names a binding of the service container.
    Container { position: usize },
    /// The call gives the user that is logged in, whose class the application configures.
    User,
    /// The call gives the repository of the entity named by the first argument, which the entity's
    /// `repositoryClass` says.
    Repository,
    /// The argument at `position` is a class that is, or implements, `base`.
    ClassArgument { base: String, position: usize },
    /// The keys of the array a method returns name something, as the events a subscriber lists.
    ReturnKeys { kind: String },
    /// The argument at `position` is an array of validation rules by field.
    Rules { position: usize },
}

struct Entry {
    /// The class that declares the method, `None` for a function.
    class: Option<String>,
    owner: Owner,
    marker: Marker,
}

fn markers() -> &'static HashMap<String, Vec<Entry>> {
    static MARKERS: OnceLock<HashMap<String, Vec<Entry>>> = OnceLock::new();
    MARKERS.get_or_init(|| {
        let mut out: HashMap<String, Vec<Entry>> = HashMap::new();
        let mut add = |owner: Owner, class: Option<&str>, name: &str, doc: Option<&Doc>| {
            let Some(doc) = doc else {
                return;
            };
            for tag in &doc.tags {
                let mut words = tag.text.split_whitespace();
                let marker = match tag.name.as_str() {
                    "key" => {
                        let Some(kind) = words.next() else {
                            continue;
                        };
                        let word = words.next();
                        let name = word.and_then(|word| word.strip_prefix('$')).map(str::to_string);
                        let position = word.and_then(|word| word.parse().ok()).unwrap_or(0);
                        Marker::Key {
                            kind: kind.to_string(),
                            position,
                            name,
                        }
                    }
                    "user" => Marker::User,
                    "repository" => Marker::Repository,
                    "class-argument" => {
                        let Some(base) = words.next() else {
                            continue;
                        };
                        Marker::ClassArgument {
                            base: base.trim_start_matches('\\').to_string(),
                            position: words.next().and_then(|word| word.parse().ok()).unwrap_or(0),
                        }
                    }
                    "return-keys" => {
                        let Some(kind) = words.next() else {
                            continue;
                        };
                        Marker::ReturnKeys { kind: kind.to_string() }
                    }
                    "container" => Marker::Container {
                        position: words.next().and_then(|word| word.parse().ok()).unwrap_or(0),
                    },
                    "rules" => Marker::Rules {
                        position: words.next().and_then(|word| word.parse().ok()).unwrap_or(0),
                    },
                    _ => continue,
                };
                out.entry(name.to_ascii_lowercase()).or_default().push(Entry {
                    class: class.map(str::to_string),
                    owner,
                    marker,
                });
            }
        };
        for (owner, symbols) in [(Owner::Laravel, laravel()), (Owner::Symfony, symfony())] {
            for function in &symbols.functions {
                add(owner, None, &function.name, function.doc.as_deref());
            }
            for class in &symbols.classes {
                for method in &class.methods {
                    add(owner, Some(&class.name), &method.name, method.doc.as_deref());
                }
            }
        }
        out
    })
}

/// The class that receives what a class does not declare, from the `@forwards` of its overlay entry:
/// the calls the framework passes on through `__call`.
pub fn forwards_of(index: &Index, class: &str) -> Option<String> {
    let frameworks = index.frameworks();
    if !(frameworks.laravel || frameworks.facades) {
        return None;
    }
    static FORWARDS: OnceLock<HashMap<String, String>> = OnceLock::new();
    FORWARDS
        .get_or_init(|| {
            let mut out = HashMap::new();
            for class in &laravel().classes {
                if let Some(tag) = class
                    .doc
                    .as_ref()
                    .and_then(|doc| doc.tags.iter().find(|tag| tag.name == "forwards"))
                {
                    out.insert(
                        class.name.to_ascii_lowercase(),
                        tag.text.trim().trim_start_matches('\\').to_string(),
                    );
                }
            }
            out
        })
        .get(&class.to_ascii_lowercase())
        .cloned()
}

/// The class whose methods name the directives of a Blade template.
pub const BLADE_COMPILER: &str = "Illuminate\\View\\Compilers\\BladeCompiler";

/// What a Blade directive names: its markers, from the method of the compiler class it is named after.
pub fn directive_markers(index: &Index, directive: &str) -> Vec<Marker> {
    markers_for(index, Some(BLADE_COMPILER), None, directive)
}

/// Whether a function or method of this name has a marker in some class, as a cheap test before the
/// call is resolved.
pub fn is_marked(name: &str) -> bool {
    markers().contains_key(&name.to_ascii_lowercase())
}

/// The markers of a function, or of a method declared in `declaring` or a class above it, or called on
/// a `receiver` that is one. Without the framework in the project nothing is marked.
pub fn markers_for(index: &Index, declaring: Option<&str>, receiver: Option<&str>, name: &str) -> Vec<Marker> {
    let frameworks = index.frameworks();
    if !frameworks.any() {
        return Vec::new();
    }
    let Some(entries) = markers().get(&name.to_ascii_lowercase()) else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|entry| match entry.owner {
            Owner::Laravel => frameworks.laravel || frameworks.facades,
            Owner::Symfony => frameworks.symfony,
        })
        .filter(|entry| match (&entry.class, declaring) {
            (None, None) => frameworks.laravel,
            (Some(class), declaring) => {
                declaring.is_some_and(|declaring| index.is_subclass_of(declaring, class))
                    || receiver.is_some_and(|receiver| index.is_subclass_of(receiver, class))
            }
            _ => false,
        })
        .map(|entry| entry.marker.clone())
        .collect()
}

/// The tag of a method, with its text.
fn tag<'a>(method: &'a Method, name: &str) -> Option<&'a str> {
    method
        .doc
        .as_ref()?
        .tags
        .iter()
        .find(|tag| tag.name == name)
        .map(|tag| tag.text.trim())
}

/// A type of a column, as far as PHP tells them apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    Int,
    String,
    Bool,
    Float,
    Array,
    Datetime,
    Mixed,
}

impl ColumnType {
    fn parse(word: &str) -> Option<ColumnType> {
        Some(match word {
            "int" => ColumnType::Int,
            "string" => ColumnType::String,
            "bool" => ColumnType::Bool,
            "float" => ColumnType::Float,
            "array" => ColumnType::Array,
            "datetime" => ColumnType::Datetime,
            "mixed" => ColumnType::Mixed,
            _ => return None,
        })
    }
}

/// A type word with an optional `?` in front.
fn typed(word: &str) -> Option<(ColumnType, bool)> {
    let (nullable, word) = match word.strip_prefix('?') {
        Some(rest) => (true, rest),
        None => (false, word),
    };
    Some((ColumnType::parse(word)?, nullable))
}

/// What a method of the schema builder does to the columns of a table.
#[derive(Clone, Debug, PartialEq)]
pub enum SchemaRule {
    /// The column named by the first argument, or by this default when there is none.
    Column {
        ty: ColumnType,
        nullable: bool,
        default_name: Option<String>,
    },
    Columns(Vec<(String, ColumnType, bool)>),
    Morphs {
        id: ColumnType,
        nullable: bool,
    },
    ForeignFor,
    Drop {
        default_name: Option<String>,
    },
    Drops(Vec<String>),
    Rename,
}

fn default_of(method: &Method) -> Option<String> {
    let default = method.callable.params.first()?.default.as_ref()?;
    Some(default.trim_matches(['\'', '"']).to_string())
}

fn schema_rule(method: &Method) -> Option<SchemaRule> {
    if let Some(text) = tag(method, "column") {
        let (ty, nullable) = typed(text.split_whitespace().next()?)?;
        return Some(SchemaRule::Column {
            ty,
            nullable,
            default_name: default_of(method),
        });
    }
    if let Some(text) = tag(method, "columns") {
        let columns = text
            .split_whitespace()
            .filter_map(|entry| {
                let (name, word) = entry.split_once(':')?;
                let (ty, nullable) = typed(word)?;
                Some((name.to_string(), ty, nullable))
            })
            .collect();
        return Some(SchemaRule::Columns(columns));
    }
    if let Some(text) = tag(method, "morphs") {
        let (id, nullable) = typed(text.split_whitespace().next()?)?;
        return Some(SchemaRule::Morphs { id, nullable });
    }
    if tag(method, "foreign-for").is_some() {
        return Some(SchemaRule::ForeignFor);
    }
    if tag(method, "drop").is_some() {
        return Some(SchemaRule::Drop {
            default_name: default_of(method),
        });
    }
    if let Some(text) = tag(method, "drops") {
        return Some(SchemaRule::Drops(text.split_whitespace().map(str::to_string).collect()));
    }
    tag(method, "rename").map(|_| SchemaRule::Rename)
}

/// The rules of the schema builder, by the lowercase name of the method.
pub fn schema_rules() -> &'static HashMap<String, SchemaRule> {
    static RULES: OnceLock<HashMap<String, SchemaRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let mut rules = HashMap::new();
        for class in laravel().classes.iter().filter(|class| is_blueprint(class)) {
            for method in &class.methods {
                if let Some(rule) = schema_rule(method) {
                    rules.insert(method.name.to_ascii_lowercase(), rule);
                }
            }
        }
        rules
    })
}

fn is_blueprint(class: &ClassDecl) -> bool {
    class.name == "Illuminate\\Database\\Schema\\Blueprint"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_schema_builder_rules() {
        let rules = schema_rules();
        assert_eq!(
            rules.get("string"),
            Some(&SchemaRule::Column {
                ty: ColumnType::String,
                nullable: false,
                default_name: None
            })
        );
        assert_eq!(
            rules.get("id"),
            Some(&SchemaRule::Column {
                ty: ColumnType::Int,
                nullable: false,
                default_name: Some("id".to_string())
            })
        );
        assert_eq!(
            rules.get("softdeletes"),
            Some(&SchemaRule::Column {
                ty: ColumnType::Datetime,
                nullable: true,
                default_name: Some("deleted_at".to_string())
            })
        );
        assert!(matches!(rules.get("timestamps"), Some(SchemaRule::Columns(columns)) if columns.len() == 2));
        assert_eq!(
            rules.get("morphs"),
            Some(&SchemaRule::Morphs {
                id: ColumnType::Int,
                nullable: false
            })
        );
        assert_eq!(rules.get("renamecolumn"), Some(&SchemaRule::Rename));
        assert!(rules.len() > 70, "{}", rules.len());
    }
}

#[cfg(test)]
mod symfony_tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn an_attribute_names_its_arguments() {
        let index = project(&[]);
        let markers = markers_for(
            &index,
            Some("Symfony\\Component\\DependencyInjection\\Attribute\\Autowire"),
            None,
            "__construct",
        );
        let kinds: Vec<String> = markers
            .iter()
            .filter_map(|marker| match marker {
                Marker::Key { kind, name, .. } => Some(format!("{kind}:{}", name.clone().unwrap_or_default())),
                _ => None,
            })
            .collect();
        assert_eq!(
            kinds,
            ["service:service", "parameter:param", "env:env", "expression:value"]
        );
    }
}
