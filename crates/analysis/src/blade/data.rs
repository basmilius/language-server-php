//! The variables a template is given, read from the places that render it: `view('users.index',
//! ['users' => $users])` and its kin with `->with()` and `compact()`, a component class's `render()`
//! with the class's public properties, `@include` and `@each` in other templates (which pass on all
//! of their own variables), `@extends`, and the attributes of `<x-...>` for an anonymous component.
//! Every place counts, and a variable is the union of what the places give it; a place whose value
//! has no type adds nothing.

use std::collections::BTreeMap;
use std::path::Path;

use php_index::framework::keys::KeyKind;
use php_index::framework::views::Views;
use php_index::{Index, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use super::scan::{Node, Tag};
use super::{Template, is_template};
use crate::ast::{self, child_of};
use crate::context::FileContext;
use crate::infer::Analyzer;
use crate::references::{Sources, find_hits};
use crate::refs::{Query, Symbol};

/// How far a template's variables are followed through the templates that include it.
const DEPTH: usize = 2;

/// The classes whose public properties the view they render is given.
const RENDERERS: &[&str] = &[
    "Illuminate\\View\\Component",
    "Illuminate\\Mail\\Mailable",
    "Livewire\\Component",
];

#[derive(Default)]
struct Found(BTreeMap<String, Vec<Type>>);

impl Found {
    fn add(&mut self, name: &str, ty: Type) {
        if matches!(ty, Type::Unknown | Type::Mixed) || !is_variable_name(name) {
            self.0.entry(name.to_string()).or_default();
            return;
        }
        let types = self.0.entry(name.to_string()).or_default();
        if !types.contains(&ty) {
            types.push(ty);
        }
    }

    fn into_given(self) -> Vec<(String, Type)> {
        self.0
            .into_iter()
            .filter(|(_, types)| !types.is_empty())
            .map(|(name, types)| (name, Type::union(types)))
            .collect()
    }
}

fn is_variable_name(name: &str) -> bool {
    super::is_variable_name(name) && name != "this"
}

/// The variables a template is given by the places that render it.
pub fn given(index: &Index, sources: &dyn Sources, path: &Path) -> Vec<(String, Type)> {
    given_at(index, sources, path, 0)
}

fn given_at(index: &Index, sources: &dyn Sources, path: &Path, depth: usize) -> Vec<(String, Type)> {
    if !index.frameworks().laravel {
        return Vec::new();
    }
    let (names, tags): (Vec<String>, Vec<String>) = {
        let views = index.section::<Views>();
        (
            views
                .views
                .iter()
                .filter(|view| view.path == path)
                .map(|view| view.name.clone())
                .collect(),
            views
                .components
                .iter()
                .filter(|component| component.class.is_none() && component.path == path)
                .map(|component| component.tag.clone())
                .collect(),
        )
    };
    let mut found = Found::default();
    for name in &names {
        let query = Query::new(
            index,
            Symbol::Key {
                kind: KeyKind::View,
                name: name.clone(),
                scope: None,
            },
        );
        for file in find_hits(index, sources, None, &query) {
            if file.path == path {
                continue;
            }
            let Some(text) = sources.text(&file.path) else {
                continue;
            };
            let starts: Vec<u32> = file.hits.iter().map(|hit| u32::from(hit.range.start())).collect();
            if is_template(&file.path) {
                if depth < DEPTH {
                    from_template(index, sources, &file.path, &text, &starts, depth, &mut found);
                }
            } else {
                from_php(index, &text, &starts, &mut found);
            }
        }
    }
    for tag in &tags {
        let query = Query::new(
            index,
            Symbol::Key {
                kind: KeyKind::Component,
                name: tag.clone(),
                scope: None,
            },
        );
        for file in find_hits(index, sources, None, &query) {
            if file.path == path || !is_template(&file.path) || depth >= DEPTH {
                continue;
            }
            let Some(text) = sources.text(&file.path) else {
                continue;
            };
            let starts: Vec<u32> = file.hits.iter().map(|hit| u32::from(hit.range.start())).collect();
            from_component_tags(index, sources, &file.path, &text, &starts, depth, &mut found);
        }
    }
    found.into_given()
}

/// The type of an expression where it is written.
fn type_of(ctx: &FileContext<'_>, node: &SyntaxNode) -> Type {
    let analyzer = ctx.analyzer(node);
    let env = analyzer.env_around(node);
    analyzer.type_of(node, &env)
}

/// What a data argument gives: the keys of an array, the variables of `compact()`, or the fields of
/// an array shape.
fn data_of(ctx: &FileContext<'_>, expression: &SyntaxNode, found: &mut Found) {
    match expression.kind() {
        ARRAY_EXPR => {
            for item in expression.children().filter(|node| node.kind() == ARRAY_ITEM) {
                let parts: Vec<SyntaxNode> = item.children().collect();
                if let [key, value] = parts.as_slice() {
                    if let Some((name, _)) = php_index::test_facts::string_value(key) {
                        found.add(&name, type_of(ctx, value));
                    }
                }
            }
        }
        CALL_EXPR if callee_name(expression).is_some_and(|name| name.eq_ignore_ascii_case("compact")) => {
            let analyzer = ctx.analyzer(expression);
            let env = analyzer.env_around(expression);
            for literal in expression.descendants().filter(|node| node.kind() == LITERAL) {
                if let Some((name, _)) = php_index::test_facts::string_value(&literal) {
                    if let Some(ty) = env.get(&name) {
                        found.add(&name, ty.clone());
                    }
                }
            }
        }
        _ => {
            for member in type_of(ctx, expression).members() {
                if let Type::Shape(fields) = member {
                    for field in fields {
                        if let Some(key) = &field.key {
                            found.add(key, field.ty.clone());
                        }
                    }
                }
            }
        }
    }
}

/// The name a call is made by: the function, or the method after `->` or `::`.
fn callee_name(call: &SyntaxNode) -> Option<String> {
    let callee = call.children().next()?;
    let name = match callee.kind() {
        NAME => callee,
        _ => callee.children().filter(|child| child.kind() == NAME).last()?,
    };
    Some(ast::text_of(&name))
}

/// The arguments of a call or `new`, with the name each is passed by.
fn arguments(owner: &SyntaxNode) -> Vec<(Option<String>, SyntaxNode)> {
    let Some(list) = child_of(owner, ARGUMENT_LIST) else {
        return Vec::new();
    };
    list.children()
        .filter(|node| node.kind() == ARGUMENT)
        .filter_map(|argument| {
            let named = argument
                .children_with_tokens()
                .any(|element| element.kind() == COLON)
                .then(|| {
                    argument
                        .children_with_tokens()
                        .filter_map(|element| element.into_token())
                        .find(|token| !token.kind().is_trivia())
                        .map(|token| token.text().to_string())
                })
                .flatten();
            Some((named, argument.children().last()?))
        })
        .collect()
}

/// The calls of a PHP file that render the view at each of the offsets.
fn from_php(index: &Index, text: &str, starts: &[u32], found: &mut Found) {
    let root = parse(text).syntax();
    let ctx = FileContext::new(index, &root);
    for start in starts {
        let Some(literal) = ast::node_at(&root, *start)
            .ancestors()
            .find(|node| node.kind() == LITERAL)
        else {
            continue;
        };
        let Some(argument) = literal.parent().filter(|node| node.kind() == ARGUMENT) else {
            continue;
        };
        let Some(owner) = argument
            .parent()
            .and_then(|list| list.parent())
            .filter(|owner| matches!(owner.kind(), CALL_EXPR | NEW_EXPR))
        else {
            continue;
        };
        let all = arguments(&owner);
        let named_data = all
            .iter()
            .find(|(name, _)| matches!(name.as_deref(), Some("with" | "data")))
            .map(|(_, value)| value.clone());
        let data = named_data.or_else(|| {
            let position = all.iter().position(|(_, value)| value == &literal)?;
            all.get(position + 1)
                .filter(|(name, _)| name.is_none())
                .map(|(_, value)| value.clone())
        });
        if let Some(data) = data {
            data_of(&ctx, &data, found);
        }
        chained_with(&ctx, &owner, found);
        from_class(index, &owner, found);
    }
}

/// `->with('user', $user)`, `->with([...])` and `->withUser($user)` after the call.
fn chained_with(ctx: &FileContext<'_>, call: &SyntaxNode, found: &mut Found) {
    let mut current = call.clone();
    while let Some(fetch) = current.parent().filter(|node| node.kind() == PROPERTY_FETCH_EXPR) {
        let Some(next) = fetch.parent().filter(|node| node.kind() == CALL_EXPR) else {
            break;
        };
        let Some(method) = fetch.children().filter(|child| child.kind() == NAME).last() else {
            break;
        };
        let method = ast::text_of(&method);
        let all = arguments(&next);
        if method.eq_ignore_ascii_case("with") {
            match all.as_slice() {
                [(_, key), (_, value)] => {
                    if let Some((name, _)) = php_index::test_facts::string_value(key) {
                        found.add(&name, type_of(ctx, value));
                    }
                }
                [(_, data)] => data_of(ctx, data, found),
                _ => {}
            }
        } else if let Some(rest) = method
            .strip_prefix("with")
            .filter(|rest| rest.starts_with(char::is_uppercase))
        {
            if let [(_, value)] = all.as_slice() {
                let mut name = rest.to_string();
                name[..1].make_ascii_lowercase();
                found.add(&name, type_of(ctx, value));
            }
        } else {
            break;
        }
        current = next;
    }
}

/// A view rendered inside a component, a mailable or a Livewire component is given the public
/// properties of that class.
fn from_class(index: &Index, call: &SyntaxNode, found: &mut Found) {
    let Some(class) = call
        .ancestors()
        .find(|node| matches!(node.kind(), CLASS_DECLARATION | ANONYMOUS_CLASS))
    else {
        return;
    };
    let Some(name) = child_of(&class, NAME) else {
        return;
    };
    let root = class.ancestors().last().unwrap_or_else(|| class.clone());
    let resolver = php_index::extract::resolver_at(&root, ast::start(&class));
    let qualified = resolver.resolve_class(&ast::text_of(&name));
    if !RENDERERS.iter().any(|base| index.is_subclass_of(&qualified, base)) {
        return;
    }
    let Some(found_class) = index.class(&qualified) else {
        return;
    };
    for property in &found_class.decl.properties {
        if property.is_static || property.visibility != php_index::Visibility::Public {
            continue;
        }
        let ty = property
            .doc_ty
            .clone()
            .or_else(|| property.ty.clone())
            .unwrap_or(Type::Unknown);
        found.add(&property.name, ty);
    }
}

/// The `@include`, `@each` and `@extends` of another template that name this one.
fn from_template(
    index: &Index,
    sources: &dyn Sources,
    path: &Path,
    text: &str,
    starts: &[u32],
    depth: usize,
    found: &mut Found,
) {
    let own = given_at(index, sources, path, depth + 1);
    let template = Template::read(index, Some(path), text, &own);
    let root = template.root();
    let ctx = FileContext::new(index, &root);
    for start in starts {
        let Some(directive) = template.nodes.iter().find_map(|node| match node {
            Node::Directive(directive) if directive.args.is_some_and(|(from, to)| from <= *start && *start <= to) => {
                Some(directive)
            }
            _ => None,
        }) else {
            continue;
        };
        let name = directive.name.to_ascii_lowercase();
        if matches!(name.as_str(), "extends" | "extendsfirst") {
            let end = template.virt.text.len() as u32;
            let env = Analyzer::new(index, &root, end).env_at(end);
            for (variable, ty) in &env.vars {
                found.add(variable, ty.clone());
            }
            continue;
        }
        let Some(at) = template.virt.to_virtual(*start) else {
            continue;
        };
        let Some(list) = ast::node_at(&root, at).ancestors().find(|node| {
            node.kind() == ARRAY_EXPR && node.parent().is_some_and(|parent| parent.kind() == EXPR_STATEMENT)
        }) else {
            continue;
        };
        let items: Vec<SyntaxNode> = list
            .children()
            .filter(|node| node.kind() == ARRAY_ITEM)
            .filter_map(|item| item.children().last())
            .collect();
        let Some(position) = items
            .iter()
            .position(|item| ast::start(item) <= at && at <= ast::end(item))
        else {
            continue;
        };
        if name == "each" {
            if let (Some(items_of), Some(variable)) = (items.get(position + 1), items.get(position + 2)) {
                if let Some((variable, _)) = php_index::test_facts::string_value(variable) {
                    let analyzer = ctx.analyzer(items_of);
                    let (_, value) = analyzer.iterable_types(&type_of(&ctx, items_of));
                    found.add(&variable, value);
                }
            }
            continue;
        }
        let analyzer = ctx.analyzer(&list);
        let env = analyzer.env_around(&list);
        for (variable, ty) in &env.vars {
            found.add(variable, ty.clone());
        }
        if let Some(data) = items.get(position + 1) {
            data_of(&ctx, data, found);
        }
    }
}

/// The attributes of the `<x-...>` tags of another template that use this component.
fn from_component_tags(
    index: &Index,
    sources: &dyn Sources,
    path: &Path,
    text: &str,
    starts: &[u32],
    depth: usize,
    found: &mut Found,
) {
    let own = given_at(index, sources, path, depth + 1);
    let template = Template::read(index, Some(path), text, &own);
    let root = template.root();
    let ctx = FileContext::new(index, &root);
    for start in starts {
        let Some(tag) = template.nodes.iter().find_map(|node| match node {
            Node::Tag(tag) if tag.name_start == *start && !tag.closing => Some(tag),
            _ => None,
        }) else {
            continue;
        };
        attributes_of(&ctx, &template, &root, tag, found);
    }
}

fn attributes_of(ctx: &FileContext<'_>, template: &Template, root: &SyntaxNode, tag: &Tag, found: &mut Found) {
    for attribute in &tag.attributes {
        if attribute.name.contains([':', '.', '@']) || attribute.name.starts_with("x-") {
            continue;
        }
        let name = camel(&attribute.name);
        if !attribute.bound {
            found.add(
                &name,
                if attribute.value.is_some() {
                    Type::String
                } else {
                    Type::Bool
                },
            );
            continue;
        }
        let Some((value_start, value_end)) = attribute.value else {
            continue;
        };
        let (Some(from), Some(to)) = (
            template.virt.to_virtual(value_start),
            template.virt.to_virtual(value_end),
        ) else {
            continue;
        };
        if from >= to {
            continue;
        }
        let Some(expression) = ast::node_at(root, from + 1)
            .ancestors()
            .find(|node| node.kind() == ARRAY_ITEM)
            .and_then(|item| item.children().last())
            .filter(|expression| ast::start(expression) >= from && ast::end(expression) <= to)
        else {
            continue;
        };
        found.add(&name, type_of(ctx, &expression));
    }
}

/// `show-view-count` is `$showViewCount`.
fn camel(name: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for character in name.chars() {
        if character == '-' {
            upper = true;
        } else if upper {
            out.extend(character.to_uppercase());
            upper = false;
        } else {
            out.push(character);
        }
    }
    out
}
