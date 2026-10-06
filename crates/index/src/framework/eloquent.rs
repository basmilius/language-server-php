//! Eloquent models: the attributes a table gives them, the relations and accessors they declare,
//! the scopes that become methods on the query builder, and the calls a model forwards to it.
//!
//! The framework declares most of this in generics (`Builder<TModel>`, `HasMany<TRelatedModel,
//! TDeclaringModel>`, `Relation::getResults(): TResult`), which the type layer already follows. What
//! it cannot know is which methods and properties a model has because of its table, its `$casts`, its
//! `scopeFoo()` methods and its relations, and that is what this module makes up.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::Section;
use super::inflect::{snake, studly, table_of};
use super::migrations::Tables;
use super::overlay::ColumnType;
use super::source::{
    Literal, array_items, class_in_attribute, literal_of, method_at, resolver_for, returned_expression, tree_of,
};
use crate::hierarchy::{Ancestor, Found};
use crate::index::{Class, Index, Origin};
use crate::model::{Attribute, Availability, Callable, ClassDecl, Doc, Method, Param, Property, Span, Visibility};
use crate::test_facts::argument_expressions;
use crate::types::{Name, Type};

pub const MODEL: &str = "Illuminate\\Database\\Eloquent\\Model";
pub const BUILDER: &str = "Illuminate\\Database\\Eloquent\\Builder";
pub const RELATION: &str = "Illuminate\\Database\\Eloquent\\Relations\\Relation";
pub const ATTRIBUTE: &str = "Illuminate\\Database\\Eloquent\\Casts\\Attribute";
const SCOPE: &str = "Illuminate\\Database\\Eloquent\\Attributes\\Scope";
const TABLE: &str = "Illuminate\\Database\\Eloquent\\Attributes\\Table";
const APPENDS: &str = "Illuminate\\Database\\Eloquent\\Attributes\\Appends";
const CASTS_ATTRIBUTES: &str = "Illuminate\\Contracts\\Database\\Eloquent\\CastsAttributes";
const SOFT_DELETES: &str = "Illuminate\\Database\\Eloquent\\SoftDeletes";
const CARBON: [&str; 2] = ["Illuminate\\Support\\Carbon", "Carbon\\Carbon"];

/// What a model class declares that Eloquent turns into members.
#[derive(Default)]
pub struct ModelInfo {
    pub table: String,
    pub timestamps: bool,
    pub soft_deletes: bool,
    pub casts: Vec<(String, Type)>,
    pub appends: Vec<String>,
    pub accessors: Vec<Accessor>,
    pub scopes: Vec<Scope>,
    pub relations: Vec<RelationMember>,
}

pub struct Accessor {
    pub name: String,
    pub ty: Option<Type>,
    pub owner: Name,
    pub name_span: Span,
    pub span: Span,
    pub write_only: bool,
}

pub struct Scope {
    pub name: String,
    pub owner: Name,
    pub method: Method,
}

pub struct RelationMember {
    pub name: String,
    pub owner: Name,
    pub ty: Type,
    pub name_span: Span,
    pub span: Span,
}

#[derive(Default)]
pub struct ModelInfos {
    found: Mutex<HashMap<String, Option<Arc<ModelInfo>>>>,
}

impl Section for ModelInfos {
    fn build(_: &Index) -> Self {
        ModelInfos::default()
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        super::is_project_php(root, path)
    }
}

/// Whether the project or a package makes relations at run time (`resolveRelationUsing`), which no
/// method shows.
#[derive(Default)]
pub struct DynamicRelations {
    pub found: bool,
}

impl Section for DynamicRelations {
    fn build(index: &Index) -> Self {
        let found = index.files().any(|file| {
            let read = match file.origin {
                Origin::Project => true,
                Origin::Vendor => file.summary().classes.iter().any(|class| {
                    class
                        .parents
                        .iter()
                        .any(|parent| parent.eq_ignore_ascii_case("Illuminate\\Support\\ServiceProvider"))
                }),
                Origin::Stub => false,
            };
            read && index
                .read_text(&file.path)
                .is_some_and(|text| text.contains("resolveRelationUsing"))
        });
        DynamicRelations { found }
    }

    // Read once: it reads every file of the project, and a relation made at run time is rare.
    fn depends_on(_: &Path, _: &Path) -> bool {
        false
    }
}

/// A relation a model declares, and the model it leads to when its type says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationInfo {
    pub name: String,
    pub owner: Name,
    pub related: Option<Name>,
    pub path: std::path::PathBuf,
    pub name_span: Span,
}

/// The relations of a model of the project, its own and its parents' and traits'. `None` when the
/// class is no model.
pub fn relations(index: &Index, model: &str) -> Option<Vec<RelationInfo>> {
    let class = index.class(model)?;
    let model_type = Type::class(class.decl.name.clone());
    let ancestors = index.ancestors(&model_type);
    model_of(&ancestors)?;
    let info = info_of(index, &class)?;
    Some(
        info.relations
            .iter()
            .map(|relation| RelationInfo {
                name: relation.name.clone(),
                owner: relation.owner.clone(),
                related: match &relation.ty {
                    Type::Class { args, .. } => args.first().and_then(|related| match related {
                        Type::Class { name, .. } => Some(name.clone()),
                        _ => None,
                    }),
                    _ => None,
                },
                path: index
                    .class(&relation.owner)
                    .map(|owner| owner.file.path.clone())
                    .unwrap_or_default(),
                name_span: relation.name_span,
            })
            .collect(),
    )
}

fn is_named(class: &ClassDecl, name: &str) -> bool {
    class.name.eq_ignore_ascii_case(name)
}

/// The class a type of ancestors stands for, when it is a model of the project.
fn model_of<'a, 'b>(ancestors: &'b [Ancestor<'a>]) -> Option<&'b Ancestor<'a>> {
    let first = ancestors.first()?;
    (!is_named(first.class.decl, MODEL) && ancestors.iter().any(|ancestor| is_named(ancestor.class.decl, MODEL)))
        .then_some(first)
}

fn info_of(index: &Index, model: &Class<'_>) -> Option<Arc<ModelInfo>> {
    let infos = index.section::<ModelInfos>();
    let key = model.decl.name.to_ascii_lowercase();
    if let Ok(found) = infos.found.lock() {
        if let Some(known) = found.get(&key) {
            return known.clone();
        }
    }
    let info = build_info(index, model).map(Arc::new);
    if let Ok(mut found) = infos.found.lock() {
        found.insert(key, info.clone());
    }
    info
}

fn owns_members(class: &Class<'_>) -> bool {
    class.file.origin != Origin::Stub && !class.decl.name.starts_with("Illuminate\\")
}

fn build_info(index: &Index, model: &Class<'_>) -> Option<ModelInfo> {
    let model_type = Type::class(model.decl.name.clone());
    let ancestors = index.ancestors(&model_type);
    let mut info = ModelInfo {
        timestamps: true,
        ..ModelInfo::default()
    };
    let mut table: Option<String> = None;
    let mut seen_cast: HashSet<String> = HashSet::new();
    let mut seen_member: HashSet<String> = HashSet::new();
    info.soft_deletes = ancestors
        .iter()
        .any(|ancestor| is_named(ancestor.class.decl, SOFT_DELETES));
    for ancestor in ancestors.iter().filter(|ancestor| owns_members(&ancestor.class)) {
        let decl = ancestor.class.decl;
        let tree = tree_of(index, &ancestor.class.file.path);
        for attribute in &decl.attributes {
            read_class_attribute(attribute, &mut table, &mut info);
        }
        if let Some(value) = property_default(decl, "table") {
            if let Some(name) = literal_string(index, ancestor.class, &value) {
                table.get_or_insert(name);
            }
        }
        if property_default(decl, "timestamps").is_some_and(|value| value.trim() == "false") {
            info.timestamps = false;
        }
        if let Some(tree) = &tree {
            for (name, ty) in casts_of(index, ancestor, tree) {
                if seen_cast.insert(name.clone()) {
                    info.casts.push((name, ty));
                }
            }
        }
        if let Some(value) = property_default(decl, "appends") {
            info.appends.extend(string_list(&value));
        }
        for method in &decl.methods {
            let lower = method.name.to_ascii_lowercase();
            if !seen_member.insert(lower.clone()) {
                continue;
            }
            if let Some(scope) = scope_of(ancestor, method) {
                info.scopes.push(scope);
            }
            if let Some(accessor) = accessor_of(index, ancestor, method, tree.as_ref()) {
                info.accessors.push(accessor);
            }
            if let Some(relation) = relation_of(index, &model_type, ancestor, method, tree.as_ref()) {
                info.relations.push(relation);
            }
        }
    }
    info.table = table.unwrap_or_else(|| table_of(&model.decl.name));
    info.appends.dedup();
    Some(info)
}

fn read_class_attribute(attribute: &Attribute, table: &mut Option<String>, info: &mut ModelInfo) {
    if attribute.name.eq_ignore_ascii_case(TABLE) {
        let named = attribute
            .args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("name"))
            .or_else(|| attribute.args.first().filter(|arg| arg.name.is_none()));
        if let Some(arg) = named {
            table.get_or_insert_with(|| unquote(&arg.value));
        }
        if attribute
            .args
            .iter()
            .any(|arg| arg.name.as_deref() == Some("timestamps") && arg.value.trim() == "false")
        {
            info.timestamps = false;
        }
    } else if attribute.name.eq_ignore_ascii_case(APPENDS) {
        for arg in &attribute.args {
            info.appends.extend(string_list(&arg.value));
        }
    }
}

fn property_default(decl: &ClassDecl, name: &str) -> Option<String> {
    decl.property(name)?.default.clone()
}

/// A string written as a literal or as a constant of a class: `'users'` and `self::TABLE`.
fn literal_string(index: &Index, class: Class<'_>, text: &str) -> Option<String> {
    let text = text.trim();
    if text.starts_with(['\'', '"']) {
        return Some(unquote(text));
    }
    let (owner, constant) = text.split_once("::")?;
    let constant = constant.trim();
    let value = match owner.trim() {
        "self" | "static" => class.decl.constant(constant)?.value.clone()?,
        other => {
            let tree = tree_of(index, &class.file.path)?;
            let resolver = crate::extract::resolver_at(&tree, class.decl.span.start);
            let found = index.class(&resolver.resolve_class(other))?;
            found.decl.constant(constant)?.value.clone()?
        }
    };
    Some(unquote(&value))
}

fn unquote(text: &str) -> String {
    text.trim().trim_matches(['\'', '"']).to_string()
}

/// The strings of an array written as text, or the single string.
fn string_list(text: &str) -> Vec<String> {
    let tree = parse(&format!("<?php {text};")).syntax();
    let Some(array) = tree.descendants().find(|node| node.kind() == ARRAY_EXPR) else {
        let single = unquote(text);
        return if single.is_empty() { Vec::new() } else { vec![single] };
    };
    array_items(&array)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(_, value)| match literal_of(&value) {
            Some(Literal::Text(text, _)) => Some(text),
            _ => None,
        })
        .collect()
}

/// The names a scope method gives: `scopeActive()` is `active`, and a method marked `#[Scope]` keeps
/// its own name.
/// The type of the first parameter of a scope method of a model, which Eloquent fills with the query
/// builder of that model whatever the method writes.
pub fn scope_builder_type(index: &Index, class: &str, method: &str) -> Option<Type> {
    if !index.frameworks().eloquent || !index.is_subclass_of(class, MODEL) {
        return None;
    }
    let found = index.find_declared_method(&Type::class(class.to_string()), method)?;
    let marked = found
        .member
        .attributes
        .iter()
        .any(|attribute| attribute.name.eq_ignore_ascii_case(SCOPE));
    let named = method
        .strip_prefix("scope")
        .and_then(|rest| rest.chars().next())
        .is_some_and(char::is_uppercase);
    (marked || named).then(|| Type::Class {
        name: BUILDER.to_string(),
        args: vec![Type::class(class.to_string())],
    })
}

fn scope_of(ancestor: &Ancestor<'_>, method: &Method) -> Option<Scope> {
    let marked = method
        .attributes
        .iter()
        .any(|attribute| attribute.name.eq_ignore_ascii_case(SCOPE));
    let name = if marked {
        method.name.clone()
    } else {
        let rest = method.name.strip_prefix("scope")?;
        let first = rest.chars().next()?;
        if !first.is_uppercase() {
            return None;
        }
        let mut chars = rest.chars();
        let head: String = chars.next()?.to_lowercase().collect();
        format!("{head}{}", chars.as_str())
    };
    Some(Scope {
        name,
        owner: ancestor.class.decl.name.clone(),
        method: method.clone(),
    })
}

fn accessor_of(index: &Index, ancestor: &Ancestor<'_>, method: &Method, tree: Option<&SyntaxNode>) -> Option<Accessor> {
    let level = index.level;
    let own = |name: String, ty: Option<Type>, write_only: bool| Accessor {
        name,
        ty,
        owner: ancestor.class.decl.name.clone(),
        name_span: method.name_span,
        span: method.span,
        write_only,
    };
    if let Some(rest) = method
        .name
        .strip_prefix("get")
        .and_then(|rest| rest.strip_suffix("Attribute"))
    {
        if !rest.is_empty() {
            let ty = method.callable.effective_return(level).cloned();
            return Some(own(snake(rest), ty, false));
        }
    }
    if let Some(rest) = method
        .name
        .strip_prefix("set")
        .and_then(|rest| rest.strip_suffix("Attribute"))
    {
        if !rest.is_empty() {
            return Some(own(snake(rest), None, true));
        }
    }
    let returns_attribute = method
        .callable
        .effective_return(level)
        .is_some_and(|ty| ty.class_names().iter().any(|name| name.eq_ignore_ascii_case(ATTRIBUTE)));
    if returns_attribute && method.callable.params.is_empty() {
        let ty = tree.and_then(|tree| attribute_getter_type(tree, method));
        return Some(own(snake(&method.name), ty, false));
    }
    None
}

/// The type the `get:` closure of `Attribute::make(...)` declares, when it declares one.
fn attribute_getter_type(tree: &SyntaxNode, method: &Method) -> Option<Type> {
    let declaration = method_at(tree, method.name_span.start)?;
    let call = returned_expression(&declaration)?;
    if call.kind() != CALL_EXPR {
        return None;
    }
    let argument_list = call.children().find(|child| child.kind() == ARGUMENT_LIST)?;
    let closure = argument_list
        .children()
        .filter(|child| child.kind() == ARGUMENT)
        .find(|argument| {
            let named = argument
                .children_with_tokens()
                .filter_map(|element| element.into_token())
                .find(|token| token.kind() == IDENT);
            named.is_none_or(|name| name.text() == "get")
        })?
        .children()
        .last()?;
    if !matches!(closure.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR) {
        return None;
    }
    let resolver = resolver_for(&closure);
    let (callable, _) = crate::extract::callable_at(&closure, &resolver, None);
    callable.ret
}

/// The relation a method declares: the type of the object it returns, with the related model bound.
fn relation_of(
    index: &Index,
    model_type: &Type,
    ancestor: &Ancestor<'_>,
    method: &Method,
    tree: Option<&SyntaxNode>,
) -> Option<RelationMember> {
    if method.is_static || method.visibility != Visibility::Public || !method.callable.params.is_empty() {
        return None;
    }
    let level = index.level;
    let declared = method.callable.effective_return(level);
    let ty = match declared {
        Some(declared) => {
            let is_relation = declared
                .class_names()
                .iter()
                .any(|name| index.is_subclass_of(name, RELATION));
            if !is_relation {
                return None;
            }
            match declared {
                Type::Class { args, .. } if !args.is_empty() => declared.clone(),
                _ => relation_from_body(index, model_type, method, tree?).unwrap_or_else(|| declared.clone()),
            }
        }
        None => relation_from_body(index, model_type, method, tree?)?,
    };
    let ty = ty.substitute(&HashMap::new(), Some(model_type), Some(&ancestor.self_name));
    Some(RelationMember {
        name: method.name.clone(),
        owner: ancestor.class.decl.name.clone(),
        ty,
        name_span: method.name_span,
        span: method.span,
    })
}

/// `return $this->hasMany(Post::class)`: what `hasMany` documents it returns, with the model it was
/// given in place of the template.
fn relation_from_body(index: &Index, model_type: &Type, method: &Method, tree: &SyntaxNode) -> Option<Type> {
    let declaration = method_at(tree, method.name_span.start)?;
    let mut call = returned_expression(&declaration)?;
    // `$this->morphMany(...)->latest()` is still the relation: the builder calls after it return it.
    loop {
        let receiver = call
            .children()
            .next()
            .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
            .and_then(|callee| callee.children().next())
            .filter(|receiver| receiver.kind() == CALL_EXPR);
        match receiver {
            Some(receiver) => call = receiver,
            None => break,
        }
    }
    if call.kind() != CALL_EXPR {
        return None;
    }
    let callee = call.children().next()?;
    if callee.kind() != PROPERTY_FETCH_EXPR || callee.children().next()?.text() != "$this" {
        return None;
    }
    let name = callee.children().find(|child| child.kind() == NAME)?.text().to_string();
    let found = index.find_declared_method(model_type, &name)?;
    let doc_ret = found.member.callable.doc_ret.clone()?;
    if !doc_ret
        .class_names()
        .iter()
        .any(|class| index.is_subclass_of(class, RELATION))
    {
        return None;
    }
    let arguments = argument_expressions(&call);
    let mut bound: HashMap<String, Type> = found.subst.as_ref().clone();
    for (position, param) in found.member.callable.params.iter().enumerate() {
        let Some(Type::ClassString(Some(inner))) = param.effective_type(index.level) else {
            continue;
        };
        let Type::Template(template) = inner.as_ref() else {
            continue;
        };
        if let Some(Literal::Class(class)) = arguments.get(position).and_then(literal_of) {
            bound.insert(template.clone(), Type::class(class));
        }
    }
    if let Some(doc) = &found.member.doc {
        for template in &doc.templates {
            bound
                .entry(template.name.clone())
                .or_insert_with(|| template.bound.clone().unwrap_or(Type::Mixed));
        }
    }
    Some(doc_ret.substitute(&bound, Some(model_type), Some(&found.self_name)))
}

/// What the casts of a class say about its attributes: the `$casts` property and the `casts()`
/// method, which Laravel 11 prefers.
fn casts_of(index: &Index, ancestor: &Ancestor<'_>, tree: &SyntaxNode) -> Vec<(String, Type)> {
    let decl = ancestor.class.decl;
    let mut out = Vec::new();
    let mut arrays: Vec<SyntaxNode> = Vec::new();
    if decl
        .property("casts")
        .and_then(|property| property.default.as_ref())
        .is_some()
    {
        if let Some(property) = tree.descendants().find(|node| {
            node.kind() == PROPERTY_ELEMENT
                && node
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .any(|token| token.kind() == VARIABLE && token.text() == "$casts")
                && node.ancestors().any(|ancestor| ancestor.kind() == CLASS_DECLARATION)
        }) {
            arrays.extend(property.children().find(|node| node.kind() == ARRAY_EXPR));
        }
    }
    if let Some(method) = decl.method("casts") {
        if let Some(declaration) = method_at(tree, method.name_span.start) {
            arrays.extend(returned_expression(&declaration).filter(|node| node.kind() == ARRAY_EXPR));
        }
    }
    for array in arrays {
        for (key, value) in array_items(&array).unwrap_or_default() {
            let Some(Literal::Text(attribute, _)) = key.as_ref().and_then(literal_of) else {
                continue;
            };
            let ty = cast_type(index, &value);
            out.push((attribute, ty));
        }
    }
    out
}

fn carbon(index: &Index) -> Type {
    for name in CARBON {
        if index.class(name).is_some() {
            return Type::class(name);
        }
    }
    Type::class("DateTimeInterface")
}

/// The type a cast gives an attribute.
fn cast_type(index: &Index, value: &SyntaxNode) -> Type {
    match literal_of(value) {
        Some(Literal::Text(text, _)) => named_cast(index, &text),
        Some(Literal::Class(class)) => class_cast(index, &class),
        None => Type::Mixed,
    }
}

fn named_cast(index: &Index, text: &str) -> Type {
    let (head, rest) = text.split_once(':').map_or((text, ""), |(head, rest)| (head, rest));
    match head.to_ascii_lowercase().as_str() {
        "int" | "integer" => Type::Int,
        "real" | "float" | "double" => Type::Float,
        "decimal" | "string" | "hashed" => Type::String,
        "bool" | "boolean" => Type::Bool,
        "object" => Type::class("stdClass"),
        "array" | "json" => Type::plain_array(),
        "collection" => Type::class("Illuminate\\Support\\Collection"),
        "date" | "datetime" | "custom_datetime" => carbon(index),
        "immutable_date" | "immutable_datetime" | "immutable_custom_datetime" => {
            let immutable = "Carbon\\CarbonImmutable";
            if index.class(immutable).is_some() {
                Type::class(immutable)
            } else {
                carbon(index)
            }
        }
        "timestamp" => Type::Int,
        "encrypted" => match rest {
            "array" | "json" => Type::plain_array(),
            "collection" => Type::class("Illuminate\\Support\\Collection"),
            "object" => Type::class("stdClass"),
            _ => Type::String,
        },
        _ => class_cast(index, text.trim_start_matches('\\')),
    }
}

fn class_cast(index: &Index, class: &str) -> Type {
    let Some(found) = index.class(class) else {
        return Type::Mixed;
    };
    if found.decl.kind == crate::model::ClassKind::Enum {
        return Type::class(found.decl.name.clone());
    }
    for ancestor in index.ancestors(&Type::class(class)) {
        if is_named(ancestor.class.decl, CASTS_ATTRIBUTES) {
            if let Some(get) = ancestor.subst.get("TGet") {
                return get.clone();
            }
        }
    }
    Type::Mixed
}

fn column_type(index: &Index, ty: ColumnType) -> Type {
    match ty {
        ColumnType::Int => Type::Int,
        ColumnType::String => Type::String,
        ColumnType::Bool => Type::Bool,
        ColumnType::Float => Type::Float,
        ColumnType::Array => Type::plain_array(),
        ColumnType::Datetime => {
            let _ = index;
            Type::String
        }
        ColumnType::Mixed => Type::Mixed,
    }
}

/// The attributes a model has from its table, casts, accessors and `$appends`: name, type, and where
/// the member is declared.
struct AttributeMember {
    name: String,
    ty: Option<Type>,
    owner: Option<Name>,
    name_span: Option<Span>,
    span: Option<Span>,
    summary: String,
    read_only: bool,
}

fn attribute_members(index: &Index, model: &Class<'_>, info: &ModelInfo) -> Vec<AttributeMember> {
    let tables = index.section::<Tables>();
    let table = tables.table(&info.table);
    let mut out: Vec<AttributeMember> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for accessor in &info.accessors {
        if seen.insert(accessor.name.clone()) {
            out.push(AttributeMember {
                name: accessor.name.clone(),
                ty: accessor.ty.clone(),
                owner: Some(accessor.owner.clone()),
                name_span: Some(accessor.name_span),
                span: Some(accessor.span),
                summary: format!("Accessor of the `{}` attribute.", accessor.name),
                read_only: false,
            });
        } else if let Some(existing) = out.iter_mut().find(|member| member.name == accessor.name) {
            if existing.ty.is_none() && !accessor.write_only {
                existing.ty = accessor.ty.clone();
            }
        }
    }
    let carbon_type = carbon(index);
    let mut push_column = |name: &str, base: Option<(ColumnType, bool)>, summary: String| {
        if !seen.insert(name.to_string()) {
            return;
        }
        let cast = info
            .casts
            .iter()
            .find(|(attribute, _)| attribute == name)
            .map(|(_, ty)| ty.clone());
        let timestamp =
            info.timestamps && matches!(name, "created_at" | "updated_at") || info.soft_deletes && name == "deleted_at";
        let (ty, nullable) = match (cast, base) {
            (Some(cast), Some((_, nullable))) => (cast, nullable),
            (Some(cast), None) => (cast, true),
            (None, Some((ColumnType::Datetime, nullable))) if timestamp => (carbon_type.clone(), nullable),
            (None, Some((column, nullable))) => (column_type(index, column), nullable),
            (None, None) if timestamp => (carbon_type.clone(), true),
            (None, None) => return,
        };
        let ty = if nullable { ty.nullable() } else { ty };
        out.push(AttributeMember {
            name: name.to_string(),
            ty: Some(ty),
            owner: None,
            name_span: None,
            span: None,
            summary,
            read_only: false,
        });
    };
    if let Some(table) = table {
        for column in &table.columns {
            push_column(
                &column.name,
                Some((column.ty, column.nullable)),
                format!("Column `{}` of the `{}` table.", column.name, info.table),
            );
        }
    }
    let casted: Vec<String> = info.casts.iter().map(|(name, _)| name.clone()).collect();
    for name in casted {
        push_column(&name, None, format!("Cast attribute `{name}`."));
    }
    for name in &info.appends {
        if seen.insert(name.clone()) {
            out.push(AttributeMember {
                name: name.clone(),
                ty: None,
                owner: None,
                name_span: None,
                span: None,
                summary: format!("Appended attribute `{name}`."),
                read_only: true,
            });
        }
    }
    let _ = model;
    out
}

fn synthetic_property(class: &ClassDecl, member: &AttributeMember) -> Property {
    Property {
        name: member.name.clone(),
        visibility: Visibility::Public,
        set_visibility: None,
        is_static: false,
        is_readonly: member.read_only,
        is_abstract: false,
        ty: None,
        doc_ty: member.ty.clone(),
        leveled: None,
        default: None,
        promoted: false,
        hooks: Vec::new(),
        doc: Some(Box::new(Doc {
            summary: member.summary.clone(),
            ..Doc::default()
        })),
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: member.name_span.unwrap_or(class.name_span),
        span: member.span.unwrap_or(class.span),
    }
}

/// The relation a method declares, read as a property: what `getResults()` gives.
fn relation_result(index: &Index, relation: &Type) -> Option<Type> {
    for ancestor in index.ancestors(relation) {
        if is_named(ancestor.class.decl, RELATION) {
            return ancestor.subst.get("TResult").cloned();
        }
    }
    None
}

pub(super) fn extend_properties<'a>(
    index: &'a Index,
    ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, Property>>,
    names: &mut HashSet<String>,
) {
    let Some(model) = model_of(ancestors) else {
        return;
    };
    let Some(info) = info_of(index, &model.class) else {
        return;
    };
    let owner_of = |owner: &Option<Name>| -> Class<'a> {
        owner.as_ref().and_then(|name| index.class(name)).unwrap_or(model.class)
    };
    for member in attribute_members(index, &model.class, &info) {
        if only.is_some_and(|only| member.name != only) || !names.insert(member.name.clone()) {
            continue;
        }
        let class = owner_of(&member.owner);
        let ancestor = ancestors
            .iter()
            .find(|ancestor| ancestor.class.decl.name == class.decl.name)
            .unwrap_or(model);
        out.push(Found {
            class,
            member: Cow::Owned(synthetic_property(class.decl, &member)),
            subst: ancestor.subst.clone(),
            self_name: ancestor.self_name.clone(),
            mixin: false,
            static_as: None,
        });
    }
    for relation in &info.relations {
        if only.is_some_and(|only| relation.name != only) || !names.insert(relation.name.clone()) {
            continue;
        }
        let Some(result) = relation_result(index, &relation.ty) else {
            continue;
        };
        let class = owner_of(&Some(relation.owner.clone()));
        let member = AttributeMember {
            name: relation.name.clone(),
            ty: Some(result),
            owner: Some(relation.owner.clone()),
            name_span: Some(relation.name_span),
            span: Some(relation.span),
            summary: format!("Relation `{}`.", relation.name),
            read_only: false,
        };
        out.push(Found {
            class,
            member: Cow::Owned(synthetic_property(class.decl, &member)),
            subst: Arc::new(HashMap::new()),
            self_name: model.self_name.clone(),
            mixin: false,
            static_as: None,
        });
    }
}

fn synthetic_method(name: &str, params: Vec<Param>, ret: Type, source: &Method) -> Method {
    Method {
        name: name.to_string(),
        visibility: Visibility::Public,
        is_static: false,
        is_abstract: false,
        is_final: false,
        callable: Callable {
            params,
            ret: None,
            doc_ret: Some(ret),
            leveled_ret: None,
            by_ref_return: false,
            is_generator: false,
            reads_all_arguments: false,
        },
        doc: source.doc.clone(),
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: source.name_span,
        span: source.span,
    }
}

fn value_param(name: &str, span: Span) -> Param {
    Param {
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
        span,
    }
}

pub(super) fn extend_methods<'a>(
    index: &'a Index,
    ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
) {
    extend_builder(index, ancestors, only, out, names);
    extend_model(index, ancestors, only, out, names);
    if only.is_some() {
        bind_relation(index, ancestors, out);
    }
}

/// The scopes and the `where{Column}` calls of the model a query builder is for.
fn extend_builder<'a>(
    index: &'a Index,
    ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
) {
    for builder in ancestors
        .iter()
        .filter(|ancestor| is_named(ancestor.class.decl, BUILDER))
    {
        let Some(Type::Class { name: model_name, .. }) = builder.subst.get("TModel") else {
            continue;
        };
        let Some(model) = index.class(model_name) else {
            continue;
        };
        let Some(info) = info_of(index, &model) else {
            continue;
        };
        for scope in &info.scopes {
            if only.is_some_and(|only| !scope.name.eq_ignore_ascii_case(only))
                || !names.insert(scope.name.to_ascii_lowercase())
            {
                continue;
            }
            let Some(owner) = index.class(&scope.owner) else {
                continue;
            };
            let params = scope.method.callable.params.iter().skip(1).cloned().collect();
            out.push(Found {
                class: owner,
                member: Cow::Owned(synthetic_method(&scope.name, params, Type::Static, &scope.method)),
                subst: Arc::new(HashMap::new()),
                self_name: owner.decl.name.clone(),
                mixin: true,
                static_as: None,
            });
        }
        let tables = index.section::<Tables>();
        let Some(table) = tables.table(&info.table) else {
            continue;
        };
        let wanted_column = |name: &str| {
            let rest = name.strip_prefix("where").or_else(|| name.strip_prefix("Where"))?;
            (!rest.is_empty()).then(|| snake(rest))
        };
        for column in &table.columns {
            let method = format!("where{}", studly(&column.name));
            if only.is_some_and(|only| !only.eq_ignore_ascii_case(&method)) {
                continue;
            }
            if let Some(only) = only {
                if wanted_column(only).as_deref() != Some(column.name.as_str()) {
                    continue;
                }
            }
            if !names.insert(method.to_ascii_lowercase()) {
                continue;
            }
            let source = Method {
                name: method.clone(),
                visibility: Visibility::Public,
                is_static: false,
                is_abstract: false,
                is_final: false,
                callable: Callable::default(),
                doc: Some(Box::new(Doc {
                    summary: format!("Where the `{}` column is `$value`.", column.name),
                    ..Doc::default()
                })),
                attributes: Vec::new(),
                availability: Availability::default(),
                name_span: model.decl.name_span,
                span: model.decl.span,
            };
            out.push(Found {
                class: builder.class,
                member: Cow::Owned(synthetic_method(
                    &method,
                    vec![value_param("value", model.decl.name_span)],
                    Type::Static,
                    &source,
                )),
                subst: builder.subst.clone(),
                self_name: builder.self_name.clone(),
                mixin: true,
                static_as: None,
            });
        }
    }
}

/// What a model does not declare goes to the builder `newQuery()` returns, as an instance call and as
/// a static one.
fn extend_model<'a>(
    index: &'a Index,
    ancestors: &[Ancestor<'a>],
    only: Option<&str>,
    out: &mut Vec<Found<'a, Method>>,
    names: &mut HashSet<String>,
) {
    let Some(model) = model_of(ancestors) else {
        return;
    };
    if only.is_none_or(|only| only.eq_ignore_ascii_case("factory")) {
        name_factory(index, model, out);
    }
    let builder = Type::Class {
        name: BUILDER.to_string(),
        args: vec![Type::class(model.class.decl.name.clone())],
    };
    let forwarded = match only {
        Some(name) => index.find_method(&builder, name).into_iter().collect(),
        None => index.methods(&builder),
    };
    for found in forwarded {
        if found.member.visibility != Visibility::Public
            || found.member.name.starts_with("__")
            || !names.insert(found.member.name.to_ascii_lowercase())
        {
            continue;
        }
        let mut method = found.member.into_owned();
        method.is_static = true;
        out.push(Found {
            class: found.class,
            member: Cow::Owned(method),
            subst: found.subst,
            self_name: found.self_name,
            mixin: true,
            static_as: Some(builder.clone()),
        });
    }
}

/// A relation method that declares `HasMany` and no more returns what its body says: `HasMany<Post,
/// $this>`, so that `$user->posts()->create(...)` is a `Post`.
fn bind_relation<'a>(index: &'a Index, ancestors: &[Ancestor<'a>], out: &mut [Found<'a, Method>]) {
    let Some(model) = model_of(ancestors) else {
        return;
    };
    let model_type = Type::class(model.class.decl.name.clone());
    for found in out.iter_mut() {
        let method = &found.member;
        if method.is_static || method.visibility != Visibility::Public || !method.callable.params.is_empty() {
            continue;
        }
        let Some(Type::Class { name, args }) = method.callable.effective_return(index.level) else {
            continue;
        };
        if !args.is_empty()
            || !index.is_subclass_of(name, RELATION)
            || found.class.decl.name.starts_with("Illuminate\\")
        {
            continue;
        }
        let Some(tree) = tree_of(index, &found.class.file.path) else {
            continue;
        };
        let Some(bound) = relation_from_body(index, &model_type, method, &tree) else {
            continue;
        };
        let mut refined = found.member.clone().into_owned();
        refined.callable.doc_ret = Some(bound);
        found.member = Cow::Owned(refined);
    }
}

const FACTORY: &str = "Illuminate\\Database\\Eloquent\\Factories\\Factory";

/// A factory that names no model in a `@extends Factory<Model>` has the one its `$model` property
/// names, else the one its name stands for, which is how `Factory` finds it at run time.
pub(crate) fn bind_factory_model(index: &Index, ancestors: &mut [Ancestor<'_>]) {
    let Some(position) = ancestors
        .iter()
        .position(|ancestor| is_named(ancestor.class.decl, FACTORY))
    else {
        return;
    };
    if position == 0 || matches!(ancestors[position].subst.get("TModel"), Some(Type::Class { .. })) {
        return;
    }
    let factory = ancestors[0].class;
    let Some(model) = factory_model(index, factory) else {
        return;
    };
    let mut subst = ancestors[position].subst.as_ref().clone();
    subst.insert("TModel".to_string(), Type::class(model));
    ancestors[position].subst = Arc::new(subst);
}

fn factory_model(index: &Index, factory: Class<'_>) -> Option<Name> {
    if let Some(default) = factory
        .decl
        .property("model")
        .and_then(|property| property.default.as_ref())
    {
        if let Some(model) = class_in_attribute(index, factory, default) {
            return Some(model);
        }
    }
    let rest = factory
        .decl
        .name
        .strip_prefix("Database\\Factories\\")?
        .strip_suffix("Factory")?;
    ["App\\Models\\", "App\\"]
        .iter()
        .map(|namespace| format!("{namespace}{rest}"))
        .find(|candidate| index.class(candidate).is_some())
}

const USE_FACTORY: &str = "Illuminate\\Database\\Eloquent\\Attributes\\UseFactory";

/// `factory()` returns the template of `HasFactory`, which a model binds with `@use HasFactory<...>`.
/// A model that does not gets the factory the framework would look for: the one `#[UseFactory]` names,
/// else `Database\Factories\<model>Factory`.
fn name_factory<'a>(index: &'a Index, model: &Ancestor<'a>, out: &mut [Found<'a, Method>]) {
    let Some(position) = out
        .iter()
        .position(|found| found.member.name.eq_ignore_ascii_case("factory") && found.member.is_static)
    else {
        return;
    };
    if matches!(out[position].subst.get("TFactory"), Some(Type::Class { .. })) {
        return;
    }
    let Some(factory) = factory_of(index, model.class) else {
        return;
    };
    let mut method = out[position].member.clone().into_owned();
    method.callable.doc_ret = Some(Type::class(factory));
    method.callable.ret = None;
    out[position].member = Cow::Owned(method);
}

fn factory_of(index: &Index, model: Class<'_>) -> Option<Name> {
    for ancestor in index.ancestors(&Type::class(model.decl.name.clone())) {
        for attribute in &ancestor.class.decl.attributes {
            if !attribute.name.eq_ignore_ascii_case(USE_FACTORY) {
                continue;
            }
            let text = attribute.args.first()?.value.trim().to_string();
            let class = text.strip_suffix("::class")?;
            let tree = tree_of(index, &ancestor.class.file.path)?;
            let resolver = crate::extract::resolver_at(&tree, ancestor.class.decl.span.start);
            return Some(resolver.resolve_class(class.trim()));
        }
    }
    let name = model.decl.name.as_str();
    let rest = name
        .strip_prefix("App\\Models\\")
        .or_else(|| name.strip_prefix("App\\"))
        .unwrap_or(name);
    let factory = format!("Database\\Factories\\{rest}Factory");
    index.class(&factory).map(|_| factory)
}

#[cfg(test)]
mod tests {
    use super::super::testing::{ELOQUENT, add, project};
    use crate::index::Index;
    use crate::types::Type;

    const USERS_TABLE: &str = r#"<?php
return new class {
    public function up() {
        Schema::create('users', function (Blueprint $table) {
            $table->id();
            $table->string('name');
            $table->string('email');
            $table->timestamp('email_verified_at')->nullable();
            $table->boolean('is_admin')->default(false);
            $table->json('settings')->nullable();
            $table->unsignedInteger('team_id');
            $table->softDeletes();
            $table->timestamps();
        });
    }
};
"#;

    const USER: &str = r#"<?php
namespace App\Models;

use App\Casts\Money;
use Illuminate\Database\Eloquent\Attributes\Scope;
use Illuminate\Database\Eloquent\Casts\Attribute;
use Illuminate\Database\Eloquent\Model;
use Illuminate\Database\Eloquent\Relations\BelongsTo;
use Illuminate\Database\Eloquent\Relations\HasMany;
use Illuminate\Database\Eloquent\SoftDeletes;

class User extends Model
{
    use SoftDeletes;

    protected $casts = ['email_verified_at' => 'datetime', 'is_admin' => 'boolean', 'balance' => Money::class];
    protected $appends = ['display_name'];

    protected function casts(): array
    {
        return ['settings' => 'array'];
    }

    public function posts(): HasMany
    {
        return $this->hasMany(Post::class);
    }

    /** @return BelongsTo<Team, $this> */
    public function team(): BelongsTo
    {
        return $this->belongsTo(Team::class);
    }

    public function latestPost()
    {
        return $this->hasMany(Post::class);
    }

    public function recentPosts(): HasMany
    {
        return $this->hasMany(Post::class)->where('a', 1)->orderBy('b');
    }

    public function getFullNameAttribute(): string {}

    protected function nickName(): Attribute
    {
        return Attribute::make(get: fn (?string $value): string => strtoupper((string) $value));
    }

    public function scopeActive($query, bool $strict = false) {}

    #[Scope]
    protected function verified($query) {}

    public function rename(): void {}
}
"#;

    const OTHERS: &[(&str, &str)] = &[
        (
            "app/Models/Post.php",
            "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Model; class Post extends Model { public $timestamps = false; }",
        ),
        (
            "app/Models/Team.php",
            "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Model; class Team extends Model {}",
        ),
        (
            "app/Casts/Money.php",
            "<?php namespace App\\Casts; use Illuminate\\Contracts\\Database\\Eloquent\\CastsAttributes; /** @implements CastsAttributes<\\App\\Money, int> */ class Money implements CastsAttributes { public function get($model, string $key, mixed $value, array $attributes) {} }",
        ),
        ("app/Money.php", "<?php namespace App; class Money {}"),
        (
            "database/migrations/2020_01_01_000000_create_users_table.php",
            USERS_TABLE,
        ),
        ("app/Models/User.php", USER),
    ];

    fn fixture() -> Index {
        let mut files = ELOQUENT.to_vec();
        files.extend_from_slice(OTHERS);
        project(&files)
    }

    fn user() -> Type {
        Type::class("App\\Models\\User")
    }

    fn property(index: &Index, name: &str) -> Option<String> {
        let found = index.find_property(&user(), name)?;
        let ty = found.member.effective_type(index.level)?;
        Some(found.resolve(ty).display(false))
    }

    #[test]
    fn columns_come_from_the_migrations_and_the_casts() {
        let index = fixture();
        assert_eq!(property(&index, "id").as_deref(), Some("int"));
        assert_eq!(property(&index, "name").as_deref(), Some("string"));
        assert_eq!(
            property(&index, "email_verified_at").as_deref(),
            Some("?Illuminate\\Support\\Carbon")
        );
        assert_eq!(property(&index, "is_admin").as_deref(), Some("bool"));
        assert_eq!(property(&index, "settings").as_deref(), Some("?array"));
        assert_eq!(
            property(&index, "created_at").as_deref(),
            Some("?Illuminate\\Support\\Carbon")
        );
        assert_eq!(
            property(&index, "deleted_at").as_deref(),
            Some("?Illuminate\\Support\\Carbon")
        );
        assert_eq!(property(&index, "team_id").as_deref(), Some("int"));
        assert_eq!(property(&index, "balance").as_deref(), Some("?App\\Money"));
        assert!(property(&index, "nope").is_none());
    }

    #[test]
    fn accessors_appends_and_relations_are_properties() {
        let index = fixture();
        assert_eq!(property(&index, "full_name").as_deref(), Some("string"));
        assert_eq!(property(&index, "nick_name").as_deref(), Some("string"));
        assert!(index.find_property(&user(), "display_name").is_some());
        assert_eq!(
            property(&index, "posts").as_deref(),
            Some("Illuminate\\Database\\Eloquent\\Collection<int, App\\Models\\Post>")
        );
        assert_eq!(property(&index, "team").as_deref(), Some("?App\\Models\\Team"));
        assert_eq!(
            property(&index, "latestPost").as_deref(),
            Some("Illuminate\\Database\\Eloquent\\Collection<int, App\\Models\\Post>")
        );
        assert_eq!(
            property(&index, "recentPosts").as_deref(),
            Some("Illuminate\\Database\\Eloquent\\Collection<int, App\\Models\\Post>")
        );
        assert!(property(&index, "rename").is_none());
    }

    #[test]
    fn a_model_forwards_what_it_lacks_to_its_builder() {
        let index = fixture();
        let where_ = index.find_method(&user(), "where").expect("forwarded");
        assert!(where_.member.is_static && where_.mixin);
        let first = index.find_method(&user(), "first").expect("forwarded");
        assert_eq!(first.subst.get("TModel"), Some(&Type::class("App\\Models\\User")));
        assert!(
            index.find_method(&user(), "orderBy").is_some(),
            "through the builder's mixin"
        );
        let save = index.find_method(&user(), "save").expect("declared");
        assert!(!save.member.is_static && !save.mixin);
    }

    #[test]
    fn scopes_and_dynamic_wheres_are_methods_of_the_builder() {
        let index = fixture();
        let builder = Type::Class {
            name: super::BUILDER.to_string(),
            args: vec![user()],
        };
        let active = index.find_method(&builder, "active").expect("a scope");
        assert_eq!(
            active.member.callable.params.len(),
            1,
            "the builder argument is dropped"
        );
        assert_eq!(active.class.decl.name, "App\\Models\\User");
        assert!(
            index.find_method(&builder, "verified").is_some(),
            "an attribute marks a scope"
        );
        assert!(index.find_method(&user(), "active").is_some(), "static call of a scope");
        assert!(index.find_method(&builder, "whereEmail").is_some());
        assert!(index.find_method(&builder, "whereIsAdmin").is_some());
        assert!(index.find_method(&builder, "whereNothing").is_none());
        assert!(
            index
                .methods(&builder)
                .iter()
                .any(|found| found.member.name == "whereTeamId")
        );
    }

    #[test]
    fn a_model_without_a_documented_factory_gets_the_one_by_convention() {
        let mut index = fixture();
        for (path, text) in [
            (
                "vendor/laravel/HasFactory.php",
                "<?php namespace Illuminate\\Database\\Eloquent\\Factories; /** @template TFactory of Factory */ trait HasFactory { /** @return TFactory */ public static function factory($count = null) {} }",
            ),
            (
                "database/factories/PostFactory.php",
                "<?php namespace Database\\Factories; class PostFactory {}",
            ),
            (
                "database/factories/OddFactory.php",
                "<?php namespace Database\\Factories; class OddFactory {}",
            ),
            (
                "app/Models/Post.php",
                "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Factories\\HasFactory; use Illuminate\\Database\\Eloquent\\Model; class Post extends Model { use HasFactory; }",
            ),
            (
                "app/Models/Odd.php",
                "<?php namespace App\\Models; use Database\\Factories\\OddFactory; use Illuminate\\Database\\Eloquent\\Attributes\\UseFactory; use Illuminate\\Database\\Eloquent\\Factories\\HasFactory; use Illuminate\\Database\\Eloquent\\Model; #[UseFactory(OddFactory::class)] class Odd extends Model { use HasFactory; }",
            ),
            (
                "app/Models/Documented.php",
                "<?php namespace App\\Models; use Illuminate\\Database\\Eloquent\\Factories\\HasFactory; use Illuminate\\Database\\Eloquent\\Model; class Documented extends Model { /** @use HasFactory<\\Database\\Factories\\PostFactory> */ use HasFactory; }",
            ),
        ] {
            super::super::testing::add(&mut index, path, text);
        }
        let ret = |class: &str| {
            let found = index
                .find_method(&Type::class(class), "factory")
                .expect("a factory method");
            found
                .member
                .callable
                .doc_ret
                .clone()
                .map(|ty| found.resolve(&ty).display(false))
        };
        assert_eq!(
            ret("App\\Models\\Post").as_deref(),
            Some("Database\\Factories\\PostFactory")
        );
        assert_eq!(
            ret("App\\Models\\Odd").as_deref(),
            Some("Database\\Factories\\OddFactory")
        );
        assert_eq!(
            ret("App\\Models\\Documented").as_deref(),
            Some("Database\\Factories\\PostFactory")
        );
    }

    #[test]
    fn a_change_to_a_migration_changes_the_columns() {
        let mut index = fixture();
        assert!(index.find_property(&user(), "age").is_none());
        add(
            &mut index,
            "database/migrations/2021_01_01_000000_add_age.php",
            "<?php Schema::table('users', function (Blueprint $table) { $table->integer('age'); });",
        );
        assert_eq!(property(&index, "age").as_deref(), Some("int"));
    }
}
