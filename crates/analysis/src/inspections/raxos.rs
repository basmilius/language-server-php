//! What the Raxos router reads from attributes and only finds wrong at run time: a `$name` in a path
//! that no parameter fills, which leaves it in the path as text, a route without a return type, which
//! the mapper refuses, and a `MapModelRelation` whose parent instance no controller above provides.
//! On a model, an `@method` the ORM answers through `__call`: one for a property that is no relation
//! throws, one with another model than its relation's is wrong, and one that matches says nothing new.

use php_index::framework::raxos::router::{CONTROLLER, Controllers, MAP_MODEL_RELATION, path_parameters, route_verb};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use php_index::Type;
use php_index::framework::raxos::orm::properties_of;

const MACRO: &str = "Raxos\\Database\\Orm\\Attribute\\Macro";
const CASTER: &str = "Raxos\\Database\\Orm\\Attribute\\Caster";
const HANDLER: &str = "Raxos\\MessageBus\\Attribute\\Handler";
const HANDLER_INTERFACE: &str = "Raxos\\Contract\\MessageBus\\HandlerInterface";
const CASTER_INTERFACE: &str = "Raxos\\Contract\\Database\\Orm\\CasterInterface";
use php_syntax::{TextRange, TextSize};

use super::Cx;
use crate::ast::{child_of, range_of, text_of, tokens};
use crate::doc_refs::{DocItemKind, doc_items};

pub(super) fn run(cx: &Cx) {
    if !cx.ready || !cx.index.frameworks().raxos {
        return;
    }
    if cx.on("model-method-mismatch") || cx.on("redundant-model-method") {
        for class in cx
            .file
            .root
            .descendants()
            .filter(|node| node.kind() == CLASS_DECLARATION)
        {
            check_model_methods(cx, &class);
        }
    }
    if cx.on("invalid-model-attribute") || cx.on("message-handler-mismatch") {
        for attribute in cx.file.root.descendants().filter(|node| node.kind() == ATTRIBUTE) {
            if cx.on("invalid-model-attribute") {
                check_model_attribute(cx, &attribute);
            }
            if cx.on("message-handler-mismatch") {
                check_message_handler(cx, &attribute);
            }
        }
    }
    if !cx.on("unknown-route-parameter") && !cx.on("route-without-return-type") {
        return;
    }
    for attribute in cx.file.root.descendants().filter(|node| node.kind() == ATTRIBUTE) {
        let Some(name) = child_of(&attribute, NAME) else {
            continue;
        };
        let analyzer = cx.file.analyzer(&attribute);
        let resolved = analyzer.resolver.resolve_class(&text_of(&name));
        let Some(class) = analyzer.class.as_ref().map(|class| class.name.clone()) else {
            continue;
        };
        if resolved.eq_ignore_ascii_case(CONTROLLER) {
            let params = parameter_names(cx, &class, "__construct");
            check_path(cx, &attribute, "prefix", &params, "the constructor");
        } else if resolved.eq_ignore_ascii_case(MAP_MODEL_RELATION) {
            check_parent_instance(cx, &attribute, &class);
        } else if route_verb(&resolved).is_some() {
            let Some(method) = attribute.ancestors().find(|node| node.kind() == METHOD_DECLARATION) else {
                continue;
            };
            let Some(method_name) = child_of(&method, NAME) else {
                continue;
            };
            let method_name_text = text_of(&method_name);
            let params = parameter_names(cx, &class, &method_name_text);
            check_path(cx, &attribute, "path", &params, &format!("'{method_name_text}()'"));
            let declared = cx.index.class(&class).and_then(|found| {
                found
                    .decl
                    .method(&method_name_text)
                    .map(|method| method.callable.ret.is_some())
            });
            if cx.on("route-without-return-type") && declared == Some(false) {
                cx.report(
                    "route-without-return-type",
                    method_name.text_range(),
                    format!("The route '{method_name_text}()' needs a return type, which the router reads"),
                    super::Fix::None,
                );
            }
        }
    }
}

/// Every `$name` of the path an attribute gives that the parameters do not have.
fn check_path(cx: &Cx, attribute: &SyntaxNode, argument: &str, params: &[String], owner: &str) {
    if !cx.on("unknown-route-parameter") {
        return;
    }
    let Some(literal) = string_argument(attribute, argument) else {
        return;
    };
    let Some((value, span)) = php_index::test_facts::string_value(&literal) else {
        return;
    };
    for (at, name) in path_parameters(&value) {
        if params.iter().any(|param| param == name) {
            continue;
        }
        let start = span.start + at as u32;
        cx.report(
            "unknown-route-parameter",
            range_of(start, start + 1 + name.len() as u32),
            format!("No parameter of {owner} is named '${name}', so the path keeps it as text"),
            super::Fix::None,
        );
    }
}

/// The first argument of `#[MapModelRelation]` names a constructor parameter of the controller or one
/// above it.
fn check_parent_instance(cx: &Cx, attribute: &SyntaxNode, class: &str) {
    if !cx.on("unknown-route-parameter") {
        return;
    }
    let Some(literal) = string_argument(attribute, "parentInstanceName") else {
        return;
    };
    let Some((value, span)) = php_index::test_facts::string_value(&literal) else {
        return;
    };
    let controllers = cx.index.section::<Controllers>();
    if controllers.get(class).is_none() {
        return;
    }
    if controllers
        .providers(class)
        .iter()
        .any(|parent| parameter_names(cx, parent, "__construct").contains(&value))
    {
        return;
    }
    cx.report(
        "unknown-route-parameter",
        range_of(span.start, span.end),
        format!("Neither this controller nor one above has a constructor parameter '${value}'"),
        super::Fix::None,
    );
}

/// The `@method` lines of a model's doc comment against its relations.
fn check_model_methods(cx: &Cx, class: &SyntaxNode) {
    let Some(doc) = tokens(class).find(|token| token.kind() == DOC_COMMENT) else {
        return;
    };
    let Some(name) = child_of(class, NAME) else {
        return;
    };
    let analyzer = cx.file.analyzer(&name);
    let Some(model) = analyzer.class.as_ref().map(|class| class.name.clone()) else {
        return;
    };
    let properties = properties_of(cx.index, &model);
    if properties.is_empty() {
        return;
    }
    let base = u32::from(doc.text_range().start());
    for item in doc_items(doc.text(), base) {
        let DocItemKind::Tag(tag) = &item.kind else {
            continue;
        };
        if tag != "method" {
            continue;
        }
        let tag_start = TextSize::from(item.start);
        let rest = &cx.text[usize::from(tag_start)..];
        let line = &rest[..rest.find(['\n', '\r']).unwrap_or(rest.len())];
        let line = &line[..line.find("*/").unwrap_or(line.len())];
        let Some(open) = line.find('(') else {
            continue;
        };
        let before = line[..open].trim_end();
        let method = &before[before
            .rfind(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
            .map_or(0, |at| at + 1)..];
        let Some((_, property)) = properties.iter().find(|(_, property)| property.answers_to(method)) else {
            continue;
        };
        let line_range = TextRange::new(tag_start, tag_start + TextSize::from(line.trim_end().len() as u32));
        match property.relation_target() {
            None if cx.on("model-method-mismatch") => cx.report(
                "model-method-mismatch",
                line_range,
                format!(
                    "'${}' is no relation, so the model throws for '{method}()'",
                    property.name
                ),
                super::Fix::None,
            ),
            Some(target) => {
                let short = target.rsplit('\\').next().unwrap_or(target);
                let documented = line
                    .find("QueryInterface<")
                    .map(|at| &line[at + "QueryInterface<".len()..])
                    .and_then(|rest| rest.split_once('>'))
                    .map(|(model, _)| model.trim().trim_start_matches('\\'));
                match documented {
                    Some(model) if model.rsplit('\\').next() == Some(short) => {
                        if cx.on("redundant-model-method") {
                            cx.report(
                                "redundant-model-method",
                                line_range,
                                format!("'{method}()' is known from the relation '${}'", property.name),
                                super::Fix::RemoveDocLine { range: line_range },
                            );
                        }
                    }
                    Some(model) if cx.on("model-method-mismatch") => cx.report(
                        "model-method-mismatch",
                        line_range,
                        format!("The relation '${}' gives '{short}', not '{model}'", property.name),
                        super::Fix::None,
                    ),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// A `#[Macro]` whose callable takes no model of this class or gives what the property cannot hold,
/// and a `#[Caster]` whose class is no caster.
fn check_model_attribute(cx: &Cx, attribute: &SyntaxNode) {
    let Some(name) = child_of(attribute, NAME) else {
        return;
    };
    let analyzer = cx.file.analyzer(attribute);
    let resolved = analyzer.resolver.resolve_class(&text_of(&name));
    let Some(model) = analyzer.class.as_ref().map(|class| class.name.clone()) else {
        return;
    };
    let Some(argument) =
        child_of(attribute, ARGUMENT_LIST).and_then(|list| list.children().find(|child| child.kind() == ARGUMENT))
    else {
        return;
    };
    let written = text_of(&argument);
    let written = match written.split_once(':') {
        Some((label, rest))
            if !rest.starts_with(':') && label.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_') =>
        {
            rest.to_string()
        }
        _ => written,
    };
    let level = cx.index.level;
    if resolved.eq_ignore_ascii_case(CASTER) {
        let Some(class) = written.trim().strip_suffix("::class") else {
            return;
        };
        let caster = analyzer.resolver.resolve_class(class.trim());
        if cx.index.class(&caster).is_some() && !cx.index.is_subclass_of(&caster, CASTER_INTERFACE) {
            cx.report(
                "invalid-model-attribute",
                argument.text_range(),
                format!("'{}' does not implement CasterInterface", short(&caster)),
                super::Fix::None,
            );
        }
        return;
    }
    if !resolved.eq_ignore_ascii_case(MACRO) {
        return;
    }
    let Some((class, method)) = written
        .trim()
        .strip_suffix("(...)")
        .and_then(|callable| callable.split_once("::"))
    else {
        return;
    };
    let macro_class = analyzer.resolver.resolve_class(class.trim());
    let Some(found) = cx.index.find_method(&Type::class(macro_class.clone()), method.trim()) else {
        return;
    };
    let takes = found
        .member
        .callable
        .params
        .first()
        .and_then(|param| param.effective_type(level))
        .map(class_members);
    if let Some(takes) = takes.filter(|takes| !takes.is_empty()) {
        if !takes.iter().any(|class| cx.index.is_subclass_of(&model, class)) {
            cx.report(
                "invalid-model-attribute",
                argument.text_range(),
                format!(
                    "'{}::{}()' does not take a '{}'",
                    short(&macro_class),
                    method.trim(),
                    short(&model)
                ),
                super::Fix::None,
            );
            return;
        }
    }
    let property = attribute
        .ancestors()
        .find(|node| node.kind() == PROPERTY_DECLARATION)
        .and_then(|declaration| declaration.children().find(|child| child.kind() == PROPERTY_ELEMENT))
        .and_then(|element| crate::ast::first_token(&element, VARIABLE))
        .map(|variable| variable.text().trim_start_matches('$').to_string());
    let holds = property
        .and_then(|property| cx.index.find_property(&Type::class(model.clone()), &property))
        .and_then(|found| found.member.ty.clone());
    let gives = found.member.callable.native_return(level).cloned();
    let (Some(holds), Some(gives)) = (holds, gives) else {
        return;
    };
    let (holds, gives) = (class_members(&holds), class_members(&gives));
    if holds.len() == 1
        && !gives.is_empty()
        && gives
            .iter()
            .any(|given| !holds.iter().any(|held| cx.index.is_subclass_of(given, held)))
    {
        cx.report(
            "invalid-model-attribute",
            argument.text_range(),
            format!(
                "'{}::{}()' gives a '{}', which the property cannot hold",
                short(&macro_class),
                method.trim(),
                gives.iter().map(|given| short(given)).collect::<Vec<_>>().join("|")
            ),
            super::Fix::None,
        );
    }
}

/// The handler `#[Handler]` names on a message implements `HandlerInterface` for that message.
fn check_message_handler(cx: &Cx, attribute: &SyntaxNode) {
    let Some(name) = child_of(attribute, NAME) else {
        return;
    };
    let analyzer = cx.file.analyzer(attribute);
    if !analyzer
        .resolver
        .resolve_class(&text_of(&name))
        .eq_ignore_ascii_case(HANDLER)
    {
        return;
    }
    let Some(message) = analyzer.class.as_ref().map(|class| class.name.clone()) else {
        return;
    };
    let Some(argument) =
        child_of(attribute, ARGUMENT_LIST).and_then(|list| list.children().find(|child| child.kind() == ARGUMENT))
    else {
        return;
    };
    let Some(class) = text_of(&argument).trim().strip_suffix("::class").map(str::to_string) else {
        return;
    };
    let handler = analyzer.resolver.resolve_class(class.trim());
    if cx.index.class(&handler).is_none() {
        return;
    }
    let interface = cx
        .index
        .ancestors(&Type::class(handler.clone()))
        .into_iter()
        .find(|ancestor| ancestor.class.decl.name.eq_ignore_ascii_case(HANDLER_INTERFACE));
    let problem = match interface {
        None => Some(format!("'{}' does not implement HandlerInterface", short(&handler))),
        Some(ancestor) => ancestor
            .class
            .decl
            .doc
            .as_ref()
            .and_then(|doc| doc.templates.first())
            .and_then(|template| ancestor.subst.get(&template.name))
            .and_then(|handled| match handled {
                Type::Class { name, .. } if !cx.index.is_subclass_of(&message, name) => Some(format!(
                    "'{}' handles '{}', not '{}'",
                    short(&handler),
                    short(name),
                    short(&message)
                )),
                _ => None,
            }),
    };
    if let Some(problem) = problem {
        cx.report(
            "message-handler-mismatch",
            argument.text_range(),
            problem,
            super::Fix::None,
        );
    }
}

/// The classes of a type when it is made of classes alone, besides `null`; empty otherwise.
fn class_members(ty: &Type) -> Vec<String> {
    let mut out = Vec::new();
    for member in ty.members() {
        match member {
            Type::Class { name, .. } => out.push(name.clone()),
            Type::Null => {}
            _ => return Vec::new(),
        }
    }
    out
}

fn short(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

fn parameter_names(cx: &Cx, class: &str, method: &str) -> Vec<String> {
    cx.index
        .class(class)
        .and_then(|found| found.decl.method(method).cloned())
        .map(|method| method.callable.params.iter().map(|param| param.name.clone()).collect())
        .unwrap_or_default()
}

/// The string literal of an attribute argument, named or the first positional one.
fn string_argument(attribute: &SyntaxNode, name: &str) -> Option<SyntaxNode> {
    let list = child_of(attribute, ARGUMENT_LIST)?;
    let arguments: Vec<SyntaxNode> = list.children().filter(|child| child.kind() == ARGUMENT).collect();
    let named = |argument: &SyntaxNode| {
        argument
            .children_with_tokens()
            .any(|element| element.kind() == COLON)
            .then(|| {
                argument
                    .children_with_tokens()
                    .filter_map(|element| element.into_token())
                    .find(|token| !token.kind().is_trivia())
                    .map(|token| token.text().to_string())
            })
            .flatten()
    };
    let argument = arguments
        .iter()
        .find(|argument| named(argument).as_deref() == Some(name))
        .or_else(|| arguments.first().filter(|argument| named(argument).is_none()))?;
    argument.children().find(|child| child.kind() == LITERAL)
}
