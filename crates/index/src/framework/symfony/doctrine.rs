//! Doctrine entities and their repositories: the repository an entity names, the fields it maps and
//! what the repository's finders give back.

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use super::super::source::class_in_attribute;
use crate::framework::Section;
use crate::framework::inflect::camel;
use crate::hierarchy::{Ancestor, Found};
use crate::index::{Class, Index, Origin};
use crate::model::{Attribute, Availability, Callable, ClassDecl, Doc, Method, Param, Span, Visibility};
use crate::types::{Name, Type};

const ENTITY_REPOSITORY: &str = "Doctrine\\ORM\\EntityRepository";
const OBJECT_REPOSITORY: &str = "Doctrine\\Persistence\\ObjectRepository";
const MAPPING: &str = "Doctrine\\ORM\\Mapping\\";

#[derive(Clone, Debug, PartialEq)]
pub enum FieldKind {
    Column,
    Id,
    /// A relation to another entity.
    Relation {
        target: Option<Name>,
        many: bool,
    },
    Embedded,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntityField {
    pub name: String,
    pub kind: FieldKind,
    pub name_span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntityInfo {
    pub class: Name,
    pub repository: Option<Name>,
    pub fields: Vec<EntityField>,
}

#[derive(Default)]
pub struct Entities {
    pub entities: Vec<EntityInfo>,
}

impl Entities {
    pub fn find(&self, class: &str) -> Option<&EntityInfo> {
        self.entities
            .iter()
            .find(|entity| entity.class.eq_ignore_ascii_case(class))
    }

    pub fn of_repository(&self, repository: &str) -> Option<&EntityInfo> {
        self.entities.iter().find(|entity| {
            entity
                .repository
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(repository))
        })
    }
}

impl Section for Entities {
    fn build(index: &Index) -> Self {
        let mut found = Entities::default();
        for class in index.class_names() {
            if class.file.origin != Origin::Project {
                continue;
            }
            let Some(loaded) = class.load() else {
                continue;
            };
            if let Some(info) = read_entity(index, loaded) {
                found.entities.push(info);
            }
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_project_php(root, path)
    }
}

fn attribute_named<'a>(attributes: &'a [Attribute], short: &str) -> Option<&'a Attribute> {
    attributes.iter().find(|attribute| {
        attribute
            .name
            .strip_prefix(MAPPING)
            .is_some_and(|name| name.eq_ignore_ascii_case(short))
    })
}

fn read_entity(index: &Index, class: Class<'_>) -> Option<EntityInfo> {
    let decl = class.decl;
    let entity = attribute_named(&decl.attributes, "Entity")?;
    let repository = entity
        .args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("repositoryClass"))
        .or_else(|| entity.args.first().filter(|arg| arg.name.is_none()))
        .and_then(|arg| class_in_attribute(index, class, &arg.value));
    let mut fields = Vec::new();
    for property in &decl.properties {
        let kind = if let Some(relation) = ["ManyToOne", "OneToOne", "OneToMany", "ManyToMany"]
            .iter()
            .find_map(|name| attribute_named(&property.attributes, name).map(|attribute| (*name, attribute)))
        {
            let (name, attribute) = relation;
            let target = attribute
                .args
                .iter()
                .find(|arg| arg.name.as_deref() == Some("targetEntity"))
                .or_else(|| attribute.args.first().filter(|arg| arg.name.is_none()))
                .and_then(|arg| class_in_attribute(index, class, &arg.value))
                .or_else(|| {
                    property
                        .effective_type(index.level)
                        .and_then(|ty| ty.class_names().into_iter().next().map(str::to_string))
                        .filter(|name| !name.ends_with("Collection"))
                });
            FieldKind::Relation {
                target,
                many: matches!(name, "OneToMany" | "ManyToMany"),
            }
        } else if attribute_named(&property.attributes, "Id").is_some() {
            FieldKind::Id
        } else if attribute_named(&property.attributes, "Column").is_some() {
            FieldKind::Column
        } else if attribute_named(&property.attributes, "Embedded").is_some() {
            FieldKind::Embedded
        } else {
            continue;
        };
        fields.push(EntityField {
            name: property.name.clone(),
            kind,
            name_span: property.name_span,
        });
    }
    Some(EntityInfo {
        class: decl.name.clone(),
        repository,
        fields,
    })
}

/// The repository class of an entity named by its class, when the entity names one.
pub fn repository_of(index: &Index, entity: &str) -> Option<Name> {
    index.section::<Entities>().find(entity)?.repository.clone()
}

/// The entity a repository serves: the `T` of its `EntityRepository<T>`, else the entity that names it.
pub fn entity_of_repository(index: &Index, repository: &Type) -> Option<Name> {
    for ancestor in index.ancestors(repository) {
        if ancestor.class.decl.name.eq_ignore_ascii_case(ENTITY_REPOSITORY) {
            if let Some(Type::Class { name, .. }) = ancestor.subst.get("T") {
                return Some(name.clone());
            }
        }
    }
    let Type::Class { name, .. } = repository else {
        return None;
    };
    index
        .section::<Entities>()
        .of_repository(name)
        .map(|entity| entity.class.clone())
}

fn is_repository(ancestors: &[Ancestor<'_>]) -> bool {
    ancestors
        .iter()
        .any(|ancestor| ancestor.class.decl.name.eq_ignore_ascii_case(ENTITY_REPOSITORY))
}

const COLLECTION: &str = "Doctrine\\Common\\Collections\\Collection";

/// A to-many relation typed `Collection` holds the entities its `targetEntity` names.
pub(crate) fn extend_properties<'a>(
    index: &'a Index,
    _ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, crate::model::Property>>,
) {
    let entities = index.section::<Entities>();
    for found in out.iter_mut() {
        if only.is_some_and(|only| found.member.name != only) {
            continue;
        }
        let Some(entity) = entities.find(&found.class.decl.name) else {
            continue;
        };
        let Some(field) = entity.fields.iter().find(|field| field.name == found.member.name) else {
            continue;
        };
        let FieldKind::Relation {
            target: Some(target),
            many: true,
        } = &field.kind
        else {
            continue;
        };
        let Some(Type::Class { name, args }) = found.member.effective_type(index.level) else {
            continue;
        };
        if !args.is_empty() || !index.is_subclass_of(name, COLLECTION) && !name.eq_ignore_ascii_case(COLLECTION) {
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

const FINDERS: [&str; 4] = ["find", "findAll", "findBy", "findOneBy"];

/// What a finder of a repository gives back, from the interface the repository implements: `T|null`
/// and `list<T>` with `T` the entity.
pub(crate) fn extend_methods<'a>(
    index: &'a Index,
    ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
) {
    let Some(first) = ancestors.first() else {
        return;
    };
    if !is_repository(ancestors) || first.class.decl.name.eq_ignore_ascii_case(ENTITY_REPOSITORY) {
        return;
    }
    let repository = Type::class(first.class.decl.name.clone());
    let Some(entity) = entity_of_repository(index, &repository) else {
        return;
    };
    let entity_type = Type::class(entity.clone());
    let interface = Type::Class {
        name: OBJECT_REPOSITORY.to_string(),
        args: vec![entity_type.clone()],
    };
    for found in out.iter_mut() {
        if !FINDERS.iter().any(|name| found.member.name.eq_ignore_ascii_case(name))
            || !found.class.decl.name.eq_ignore_ascii_case(ENTITY_REPOSITORY)
        {
            continue;
        }
        let Some(documented) = index
            .find_declared_method(&interface, &found.member.name)
            .and_then(|from| from.member.callable.doc_ret.clone().map(|ret| from.resolve(&ret)))
        else {
            continue;
        };
        let mut refined = found.member.clone().into_owned();
        refined.callable.doc_ret = Some(documented);
        refined.callable.ret = None;
        found.member = Cow::Owned(refined);
    }
    magic_finders(index, &entity, only, first, out, names);
}

/// `findByEmail($email)`, `findOneByEmail($email)` and `countByEmail($email)`, which the repository
/// answers through `__call` when the entity has the field.
fn magic_finders<'a>(
    index: &'a Index,
    entity: &str,
    only: Option<&str>,
    first: &Ancestor<'a>,
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
) {
    let Some(class) = index.class(entity) else {
        return;
    };
    let entity_type = Type::class(entity.to_string());
    let info = index.section::<Entities>();
    let fields: Vec<String> = match info.find(entity) {
        Some(info) => info.fields.iter().map(|field| field.name.clone()).collect(),
        None => class
            .decl
            .properties
            .iter()
            .map(|property| property.name.clone())
            .collect(),
    };
    for field in fields {
        let studly = {
            let mut chars = field.chars();
            chars
                .next()
                .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
                .unwrap_or_default()
        };
        let variants: [(&str, Type); 3] = [
            ("findBy", Type::List(Box::new(entity_type.clone()))),
            ("findOneBy", entity_type.clone().nullable()),
            ("countBy", Type::Int),
        ];
        for (prefix, ret) in variants {
            let name = format!("{prefix}{studly}");
            if only.is_some_and(|only| !only.eq_ignore_ascii_case(&name)) || !names.insert(name.to_ascii_lowercase()) {
                continue;
            }
            out.push(Found {
                class: first.class,
                member: Cow::Owned(magic_method(&name, ret, &field, class.decl)),
                subst: Arc::new(Default::default()),
                self_name: first.self_name.clone(),
                mixin: true,
                static_as: None,
            });
        }
    }
    let _ = camel;
}

fn magic_method(name: &str, ret: Type, field: &str, entity: &ClassDecl) -> Method {
    let param = |name: &str| Param {
        name: name.to_string(),
        ty: None,
        doc_ty: Some(Type::Mixed),
        leveled: None,
        default: None,
        variadic: false,
        by_ref: false,
        promoted: None,
        description: String::new(),
        attributes: Vec::new(),
        availability: Availability::default(),
        span: entity.name_span,
    };
    Method {
        name: name.to_string(),
        visibility: Visibility::Public,
        is_static: false,
        is_abstract: false,
        is_final: false,
        callable: Callable {
            params: vec![param("value")],
            ret: None,
            doc_ret: Some(ret),
            leveled_ret: None,
            by_ref_return: false,
            is_generator: false,
            reads_all_arguments: false,
        },
        doc: Some(Box::new(Doc {
            summary: format!("By the `{field}` field of {}.", crate::types::short_name(&entity.name)),
            ..Doc::default()
        })),
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: entity.name_span,
        span: entity.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    const FRAMEWORK: &[(&str, &str)] = &[
        (
            "vendor/orm/Entity.php",
            "<?php namespace Doctrine\\ORM\\Mapping; #[\\Attribute] class Entity { public function __construct(public ?string $repositoryClass = null) {} } #[\\Attribute] class Column {} #[\\Attribute] class Id {} #[\\Attribute] class ManyToOne {} #[\\Attribute] class OneToMany {}",
        ),
        (
            "vendor/orm/ObjectRepository.php",
            "<?php namespace Doctrine\\Persistence;\n/** @template-covariant T of object */\ninterface ObjectRepository {\n    /** @return T|null */\n    public function find($id);\n    /** @return T[] */\n    public function findAll();\n    /** @return T[] */\n    public function findBy(array $criteria, ?array $orderBy = null, $limit = null, $offset = null);\n    /** @return T|null */\n    public function findOneBy(array $criteria);\n}",
        ),
        (
            "vendor/orm/EntityRepository.php",
            "<?php namespace Doctrine\\ORM;\n/**\n * @template T of object\n * @template-implements \\Doctrine\\Persistence\\ObjectRepository<T>\n */\nclass EntityRepository implements \\Doctrine\\Persistence\\ObjectRepository {\n    /** @return object|null The entity */\n    public function find(mixed $id, $lockMode = null, $lockVersion = null): object|null {}\n    public function findAll(): array {}\n    public function findBy(array $criteria, ?array $orderBy = null, $limit = null, $offset = null): array {}\n    public function findOneBy(array $criteria, ?array $orderBy = null): object|null {}\n    public function __call(string $method, array $arguments): mixed {}\n}",
        ),
        (
            "vendor/orm/ServiceEntityRepository.php",
            "<?php namespace Doctrine\\Bundle\\DoctrineBundle\\Repository;\n/**\n * @template T of object\n * @template-extends \\Doctrine\\ORM\\EntityRepository<T>\n */\nclass ServiceEntityRepository extends \\Doctrine\\ORM\\EntityRepository {}",
        ),
    ];

    fn with_entities() -> crate::index::Index {
        let mut files = FRAMEWORK.to_vec();
        files.extend_from_slice(&[
            (
                "src/Entity/User.php",
                "<?php namespace App\\Entity;\nuse App\\Repository\\UserRepository;\nuse Doctrine\\ORM\\Mapping as ORM;\n#[ORM\\Entity(repositoryClass: UserRepository::class)]\nclass User {\n    #[ORM\\Id] #[ORM\\Column] private ?int $id = null;\n    #[ORM\\Column] private string $email;\n    #[ORM\\Column] private string $firstName;\n    #[ORM\\OneToMany(targetEntity: Post::class, mappedBy: 'author')] private $posts;\n}",
            ),
            ("src/Entity/Post.php", "<?php namespace App\\Entity; use Doctrine\\ORM\\Mapping as ORM; #[ORM\\Entity] class Post { #[ORM\\ManyToOne(targetEntity: User::class)] private ?User $author = null; }"),
            (
                "src/Repository/UserRepository.php",
                "<?php namespace App\\Repository;\nuse App\\Entity\\User;\nuse Doctrine\\Bundle\\DoctrineBundle\\Repository\\ServiceEntityRepository;\n/** @extends ServiceEntityRepository<User> */\nclass UserRepository extends ServiceEntityRepository {}",
            ),
        ]);
        project(&files)
    }

    #[test]
    fn an_entity_names_its_repository_and_its_fields() {
        let index = with_entities();
        assert_eq!(
            repository_of(&index, "App\\Entity\\User").as_deref(),
            Some("App\\Repository\\UserRepository")
        );
        assert_eq!(repository_of(&index, "App\\Entity\\Post"), None);
        let entities = index.section::<Entities>();
        let user = entities.find("App\\Entity\\User").expect("an entity");
        let names: Vec<&str> = user.fields.iter().map(|field| field.name.as_str()).collect();
        assert_eq!(names, ["id", "email", "firstName", "posts"]);
        assert_eq!(
            user.fields[3].kind,
            FieldKind::Relation {
                target: Some("App\\Entity\\Post".to_string()),
                many: true
            }
        );
    }

    #[test]
    fn a_collection_of_a_relation_holds_the_target_entity() {
        let mut index = with_entities();
        crate::framework::testing::add(
            &mut index,
            "vendor/orm/Collection.php",
            "<?php namespace Doctrine\\Common\\Collections; /** @template TKey @template T */ interface Collection {}",
        );
        crate::framework::testing::add(
            &mut index,
            "src/Entity/Author.php",
            "<?php namespace App\\Entity; use Doctrine\\Common\\Collections\\Collection; use Doctrine\\ORM\\Mapping as ORM; #[ORM\\Entity] class Author { #[ORM\\OneToMany(targetEntity: Post::class, mappedBy: 'author')] private Collection $posts; }",
        );
        let found = index
            .find_property(&Type::class("App\\Entity\\Author"), "posts")
            .expect("a property");
        let ty = found.member.effective_type(index.level).expect("a type");
        assert_eq!(
            ty.display(false),
            "Doctrine\\Common\\Collections\\Collection<int, App\\Entity\\Post>"
        );
    }

    #[test]
    fn the_finders_of_a_repository_give_its_entity() {
        let index = with_entities();
        let repository = Type::class("App\\Repository\\UserRepository");
        let shown = |name: &str| {
            let found = index.find_method(&repository, name).expect("a method");
            found
                .member
                .callable
                .effective_return(index.level)
                .map(|ty| found.resolve(ty).display(false))
        };
        assert_eq!(shown("find").as_deref(), Some("?App\\Entity\\User"));
        assert_eq!(shown("findOneBy").as_deref(), Some("?App\\Entity\\User"));
        assert_eq!(shown("findAll").as_deref(), Some("list<App\\Entity\\User>"));
        assert_eq!(shown("findByEmail").as_deref(), Some("list<App\\Entity\\User>"));
        assert_eq!(shown("findOneByFirstName").as_deref(), Some("?App\\Entity\\User"));
        assert_eq!(shown("countByEmail").as_deref(), Some("int"));
        assert!(index.find_method(&repository, "findByNothing").is_none());
    }
}
