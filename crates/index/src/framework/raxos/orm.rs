//! Raxos ORM models as the structure generator reads them: which properties are columns (under
//! which key and alias), relations (to which model) and macros. A to-many relation types the
//! `ModelArrayList` of its property, and every relation is also a method that gives its query,
//! which the model answers through `__call`.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use super::super::source::class_in_attribute;
use crate::framework::Section;
use crate::hierarchy::{Ancestor, Found};
use crate::index::{Index, Origin};
use crate::model::{Attribute, Availability, Callable, Doc, Method, Property, Span, Visibility};
use crate::types::{Name, Type};

pub const MODEL: &str = "Raxos\\Database\\Orm\\Model";
pub const MODEL_ARRAY_LIST: &str = "Raxos\\Database\\Orm\\ModelArrayList";
pub const QUERY: &str = "Raxos\\Contract\\Database\\Query\\QueryInterface";
const ATTRIBUTE: &str = "Raxos\\Database\\Orm\\Attribute\\";
const TO_MANY: [&str; 3] = ["HasMany", "HasManyThrough", "BelongsToMany"];
const TO_ONE: [&str; 4] = ["BelongsTo", "HasOne", "HasOneThrough", "BelongsToThrough"];
const COLUMNS: [&str; 3] = ["Column", "PrimaryKey", "ForeignKey"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelPropertyKind {
    /// Stored in the column `key`.
    Column {
        key: String,
        computed: bool,
    },
    /// The model it points at is the one the attribute names for a to-many relation, else the
    /// type of the property.
    Relation {
        target: Option<Name>,
        many: bool,
    },
    Macro,
    Embedded,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelProperty {
    pub name: String,
    /// The other name `#[Alias]` gives it, which is also its key in JSON.
    pub alias: Option<String>,
    pub kind: ModelPropertyKind,
    pub name_span: Span,
}

impl ModelProperty {
    /// The names the structure finds it under: its own, its alias and its column key.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        let key = match &self.kind {
            ModelPropertyKind::Column { key, .. } => Some(key.as_str()),
            _ => None,
        };
        std::iter::once(self.name.as_str())
            .chain(self.alias.as_deref())
            .chain(key)
    }

    pub fn answers_to(&self, name: &str) -> bool {
        self.names().any(|candidate| candidate == name)
    }

    pub fn is_column(&self) -> bool {
        matches!(self.kind, ModelPropertyKind::Column { .. })
    }

    pub fn relation_target(&self) -> Option<&Name> {
        match &self.kind {
            ModelPropertyKind::Relation { target, .. } => target.as_ref(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelInfo {
    pub class: Name,
    pub path: std::path::PathBuf,
    /// What `#[Table]` names.
    pub table: Option<String>,
    pub properties: Vec<ModelProperty>,
}

/// The models of the project, by lowercase class name.
#[derive(Default)]
pub struct Models {
    by_class: HashMap<String, ModelInfo>,
}

impl Models {
    pub fn get(&self, class: &str) -> Option<&ModelInfo> {
        self.by_class.get(&class.trim_start_matches('\\').to_ascii_lowercase())
    }
}

impl Section for Models {
    fn build(index: &Index) -> Self {
        let mut found = Models::default();
        for class in index.class_names() {
            if class.file.origin != Origin::Project {
                continue;
            }
            let Some(loaded) = class.load() else {
                continue;
            };
            let properties: Vec<ModelProperty> = loaded
                .decl
                .properties
                .iter()
                .filter_map(|property| read_property(index, loaded, property))
                .collect();
            let table = attribute(&loaded.decl.attributes, "Table")
                .and_then(|table| positional_or_named(table, "name"))
                .and_then(|value| unquote(&value));
            if properties.is_empty() && table.is_none() {
                continue;
            }
            found.by_class.insert(
                loaded.decl.name.to_ascii_lowercase(),
                ModelInfo {
                    class: loaded.decl.name.clone(),
                    path: loaded.file.path.clone(),
                    table,
                    properties,
                },
            );
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_project_php(root, path)
    }
}

/// The ORM properties of a model and the models above it, the nearest first.
pub fn properties_of(index: &Index, class: &str) -> Vec<(Arc<str>, ModelProperty)> {
    let models = index.section::<Models>();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for ancestor in index.ancestors(&Type::class(class.to_string())) {
        let Some(info) = models.get(&ancestor.class.decl.name) else {
            continue;
        };
        let declaring: Arc<str> = Arc::from(info.class.as_str());
        for property in &info.properties {
            if seen.insert(property.name.clone()) {
                out.push((declaring.clone(), property.clone()));
            }
        }
    }
    out
}

/// The model a type holds: the model itself, the one of a `ModelArrayList<K, M>` or of a
/// `QueryInterface<M>`.
pub fn model_of(index: &Index, ty: &Type) -> Option<Name> {
    ty.members().iter().find_map(|member| {
        let Type::Class { name, args } = member else {
            return None;
        };
        let generic = [MODEL_ARRAY_LIST, QUERY]
            .iter()
            .any(|holder| name.eq_ignore_ascii_case(holder) || index.is_subclass_of(name, holder));
        if generic {
            return args.last().and_then(|arg| model_of(index, arg));
        }
        index.is_subclass_of(name, MODEL).then(|| name.clone())
    })
}

fn read_property(index: &Index, class: crate::index::Class<'_>, property: &Property) -> Option<ModelProperty> {
    let alias = attribute(&property.attributes, "Alias");
    let explicit_alias = alias
        .and_then(|alias| positional_or_named(alias, "alias"))
        .and_then(|value| unquote(&value));
    let kind = if let Some((name, relation)) = TO_MANY
        .iter()
        .chain(TO_ONE.iter())
        .find_map(|name| attribute(&property.attributes, name).map(|found| (*name, found)))
    {
        let many = TO_MANY.contains(&name);
        let target = if many {
            positional_or_named(relation, "referenceModel").and_then(|value| class_in_attribute(index, class, &value))
        } else {
            property
                .effective_type(index.level)
                .and_then(|ty| ty.class_names().first().map(|name| name.to_string()))
        };
        ModelPropertyKind::Relation { target, many }
    } else if attribute(&property.attributes, "Macro").is_some() {
        ModelPropertyKind::Macro
    } else if attribute(&property.attributes, "Embedded").is_some() {
        ModelPropertyKind::Embedded
    } else {
        let column = COLUMNS.iter().find_map(|name| attribute(&property.attributes, name))?;
        let key = positional_or_named(column, "key")
            .and_then(|value| unquote(&value))
            .unwrap_or_else(|| property.name.clone());
        ModelPropertyKind::Column {
            key,
            computed: attribute(&property.attributes, "Computed").is_some(),
        }
    };
    // An `#[Alias]` without a name makes a column known by its key, the way the generator does.
    let alias = match (&kind, alias, explicit_alias) {
        (_, _, Some(alias)) => Some(alias),
        (ModelPropertyKind::Column { key, .. }, Some(_), None) => Some(key.clone()),
        _ => None,
    };
    Some(ModelProperty {
        name: property.name.clone(),
        alias,
        kind,
        name_span: property.name_span,
    })
}

fn attribute<'a>(attributes: &'a [Attribute], short: &str) -> Option<&'a Attribute> {
    attributes.iter().find(|attribute| {
        attribute
            .name
            .strip_prefix(ATTRIBUTE)
            .is_some_and(|name| name.eq_ignore_ascii_case(short))
    })
}

/// The first positional argument of an attribute, or the one of this name.
fn positional_or_named(attribute: &Attribute, name: &str) -> Option<String> {
    attribute
        .args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(name))
        .or_else(|| attribute.args.first().filter(|arg| arg.name.is_none()))
        .map(|arg| arg.value.clone())
}

fn unquote(text: &str) -> Option<String> {
    let text = text.trim();
    let quoted = text.len() >= 2
        && (text.starts_with('\'') && text.ends_with('\'') || text.starts_with('"') && text.ends_with('"'));
    quoted.then(|| text[1..text.len() - 1].to_string())
}

pub(crate) fn extend_properties<'a>(index: &'a Index, only: Option<&str>, out: &mut [Found<'a, Property>]) {
    let models = index.section::<Models>();
    for found in out.iter_mut() {
        if only.is_some_and(|only| found.member.name != only) {
            continue;
        }
        let Some(target) = models.get(&found.class.decl.name).and_then(|info| {
            info.properties
                .iter()
                .find(|property| property.name == found.member.name)
                .filter(|property| matches!(property.kind, ModelPropertyKind::Relation { many: true, .. }))
                .and_then(ModelProperty::relation_target)
        }) else {
            continue;
        };
        let Some(Type::Class { name, args }) = found.member.effective_type(index.level) else {
            continue;
        };
        if !args.is_empty()
            || !name.eq_ignore_ascii_case(MODEL_ARRAY_LIST) && !index.is_subclass_of(name, MODEL_ARRAY_LIST)
        {
            continue;
        }
        let mut refined = found.member.clone().into_owned();
        refined.doc_ty = Some(Type::Class {
            name: name.clone(),
            args: vec![Type::Int, Type::class(target.clone())],
        });
        found.member = Cow::Owned(refined);
    }
}

/// A method for every relation, named like its property, that gives the relation's query.
pub(crate) fn extend_methods<'a>(
    index: &'a Index,
    ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
) {
    let models = index.section::<Models>();
    for ancestor in ancestors {
        let Some(info) = models.get(&ancestor.class.decl.name) else {
            continue;
        };
        for property in &info.properties {
            let Some(target) = property.relation_target() else {
                continue;
            };
            if only.is_some_and(|only| !only.eq_ignore_ascii_case(&property.name))
                || !names.insert(property.name.to_ascii_lowercase())
            {
                continue;
            }
            let span = ancestor
                .class
                .decl
                .properties
                .iter()
                .find(|declared| declared.name == property.name)
                .map_or(property.name_span, |declared| declared.span);
            out.push(Found {
                class: ancestor.class,
                member: Cow::Owned(relation_method(property, target, span)),
                subst: Arc::new(HashMap::new()),
                self_name: ancestor.self_name.clone(),
                mixin: false,
                static_as: None,
            });
        }
    }
}

fn relation_method(property: &ModelProperty, target: &str, span: Span) -> Method {
    Method {
        name: property.name.clone(),
        visibility: Visibility::Public,
        is_static: false,
        is_abstract: false,
        is_final: false,
        callable: Callable {
            params: Vec::new(),
            ret: None,
            doc_ret: Some(Type::Class {
                name: QUERY.to_string(),
                args: vec![Type::class(target.to_string())],
            }),
            leveled_ret: None,
            by_ref_return: false,
            is_generator: false,
            reads_all_arguments: false,
        },
        doc: Some(Box::new(Doc {
            summary: format!("The query of the `{}` relation.", property.name),
            ..Doc::default()
        })),
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: property.name_span,
        span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::{RAXOS, project};

    fn with_models() -> Index {
        let mut files = RAXOS.to_vec();
        files.extend_from_slice(&[
            (
                "src/AppTeam.php",
                "<?php namespace App;\nuse Raxos\\Database\\Orm\\{Model, ModelArrayList};\nuse Raxos\\Database\\Orm\\Attribute\\{Alias, BelongsTo, BelongsToMany, Column, HasMany, Macro, PrimaryKey, Table};\n#[Table('app_team')]\nclass AppTeam extends Model {\n    #[PrimaryKey] #[Alias] public int $id;\n    #[Column('created_on')] #[Alias('createdOn')] public string $createdOn;\n    #[Column] public string $name;\n    #[BelongsTo] public ?Event $event;\n    #[HasMany(Scan::class)] public ModelArrayList $scans;\n    #[BelongsToMany(referenceModel: Product::class, linkingTable: 'app_team_product')] public ModelArrayList $products;\n    /** @var ModelArrayList<int, Model> */\n    #[HasMany(Scan::class)] public ModelArrayList $documented;\n    #[Macro(AppTeamMacro::label(...))] public string $label;\n    public ModelArrayList $plain;\n}",
            ),
            ("src/Event.php", "<?php namespace App; class Event extends \\Raxos\\Database\\Orm\\Model {}"),
            (
                "src/SpecialTeam.php",
                "<?php namespace App;\nuse Raxos\\Database\\Orm\\Attribute\\Column;\nclass SpecialTeam extends AppTeam {\n    #[Column] public string $badge;\n}",
            ),
        ]);
        project(&files)
    }

    fn property_type(index: &Index, name: &str) -> String {
        index
            .find_property(&Type::class("App\\AppTeam"), name)
            .and_then(|found| found.member.effective_type(index.level).map(|ty| ty.display(true)))
            .unwrap_or_default()
    }

    fn method_return(index: &Index, class: &str, name: &str) -> Option<String> {
        index
            .find_method(&Type::class(class), name)
            .and_then(|found| found.member.callable.doc_ret.clone())
            .map(|ty| ty.display(true))
    }

    #[test]
    fn a_to_many_relation_types_its_model_array_list() {
        let index = with_models();
        assert_eq!(property_type(&index, "scans"), "ModelArrayList<int, Scan>");
        assert_eq!(property_type(&index, "products"), "ModelArrayList<int, Product>");
        assert_eq!(property_type(&index, "documented"), "ModelArrayList<int, Model>");
        assert_eq!(property_type(&index, "plain"), "ModelArrayList");
    }

    #[test]
    fn a_relation_is_a_method_that_gives_its_query() {
        let index = with_models();
        assert_eq!(
            method_return(&index, "App\\AppTeam", "scans").as_deref(),
            Some("QueryInterface<Scan>")
        );
        assert_eq!(
            method_return(&index, "App\\AppTeam", "event").as_deref(),
            Some("QueryInterface<Event>")
        );
        assert_eq!(
            method_return(&index, "App\\SpecialTeam", "products").as_deref(),
            Some("QueryInterface<Product>")
        );
        assert_eq!(method_return(&index, "App\\AppTeam", "name"), None);
        assert_eq!(method_return(&index, "App\\AppTeam", "label"), None);
    }

    #[test]
    fn columns_are_known_by_name_alias_and_key() {
        let index = with_models();
        let properties = properties_of(&index, "App\\SpecialTeam");
        let names = |name: &str| -> Vec<String> {
            properties
                .iter()
                .find(|(_, property)| property.name == name)
                .map(|(_, property)| property.names().map(str::to_string).collect())
                .unwrap_or_default()
        };
        assert_eq!(names("id"), ["id", "id", "id"]);
        assert_eq!(names("createdOn"), ["createdOn", "createdOn", "created_on"]);
        assert_eq!(names("badge"), ["badge", "badge"]);
        assert!(
            properties
                .iter()
                .any(|(declaring, property)| property.name == "scans" && &**declaring == "App\\AppTeam")
        );
        let models = index.section::<Models>();
        assert_eq!(
            models.get("App\\AppTeam").and_then(|info| info.table.as_deref()),
            Some("app_team")
        );
        assert_eq!(
            model_of(
                &index,
                &Type::Class {
                    name: MODEL_ARRAY_LIST.into(),
                    args: vec![Type::Int, Type::class("App\\Event")]
                }
            )
            .as_deref(),
            Some("App\\Event")
        );
    }
}
