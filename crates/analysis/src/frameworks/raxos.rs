//! The model a Raxos key string is read against: the model a call is made on or holds, or the target
//! of the relation a `#[Visible]` sits on, then the target of each relation a nested array names.

use php_index::framework::raxos::orm::{model_of, properties_of};
use php_index::framework::raxos::router::{Controllers, MAP_MODEL_RELATION};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::keys::Callee;
use crate::ast;
use crate::infer::Analyzer;

pub(crate) fn model_scope(
    analyzer: &Analyzer<'_>,
    owner: &SyntaxNode,
    callee: &Callee,
    path: &[String],
) -> Option<String> {
    let map_model_relation = callee
        .declaring
        .as_deref()
        .is_some_and(|declaring| declaring.eq_ignore_ascii_case(MAP_MODEL_RELATION));
    let mut model = if map_model_relation {
        parent_instance_model(analyzer, owner)?
    } else if owner.kind() == ATTRIBUTE {
        relation_under_attribute(analyzer, owner)?
    } else {
        model_of(analyzer.index, callee.receiver_type.as_ref()?)?
    };
    for key in path {
        model = relation_target(analyzer, &model, key)?;
    }
    Some(model)
}

/// The model of the constructor parameter of the controller or one above that the first argument of
/// `#[MapModelRelation('merchant', 'buyers')]` names.
fn parent_instance_model(analyzer: &Analyzer<'_>, attribute: &SyntaxNode) -> Option<String> {
    let list = ast::child_of(attribute, ARGUMENT_LIST)?;
    let first = list.children().find(|child| child.kind() == ARGUMENT)?;
    let literal = first.children().find(|child| child.kind() == LITERAL)?;
    let (name, _) = php_index::test_facts::string_value(&literal)?;
    let class = analyzer.class.as_ref()?;
    let level = analyzer.index.level;
    analyzer
        .index
        .section::<Controllers>()
        .providers(&class.name)
        .iter()
        .find_map(|parent| {
            let found = analyzer.index.class(parent)?;
            let constructor = found.decl.method("__construct")?;
            let param = constructor.callable.params.iter().find(|param| param.name == name)?;
            model_of(analyzer.index, param.effective_type(level)?)
        })
}

/// The model the relation property an attribute is written on points at.
fn relation_under_attribute(analyzer: &Analyzer<'_>, attribute: &SyntaxNode) -> Option<String> {
    let declaration = attribute.ancestors().find(|node| node.kind() == PROPERTY_DECLARATION)?;
    let element = declaration.children().find(|child| child.kind() == PROPERTY_ELEMENT)?;
    let name = ast::first_token(&element, VARIABLE)?;
    let class = analyzer.class.as_ref()?;
    relation_target(analyzer, &class.name, name.text().trim_start_matches('$'))
}

fn relation_target(analyzer: &Analyzer<'_>, model: &str, key: &str) -> Option<String> {
    properties_of(analyzer.index, model)
        .into_iter()
        .find(|(_, property)| property.answers_to(key))
        .and_then(|(_, property)| property.relation_target().cloned())
}

/// The whole paths of the route or controller whose Raxos router attribute holds an offset, as a
/// fenced block, with the range of the attribute.
pub fn route_hover(analyzer: &Analyzer<'_>, offset: u32) -> Option<(String, php_syntax::TextRange)> {
    if !analyzer.index.frameworks().raxos {
        return None;
    }
    let token = analyzer
        .root
        .token_at_offset(php_syntax::TextSize::from(offset))
        .right_biased()?;
    let attribute = token.parent_ancestors().find(|node| node.kind() == ATTRIBUTE)?;
    let name = ast::child_of(&attribute, NAME)?;
    let resolved = analyzer.resolver.resolve_class(&ast::text_of(&name));
    let class = analyzer.class.as_ref()?;
    let controllers = analyzer.index.section::<Controllers>();
    let info = controllers.get(&class.name)?;
    let paths: Vec<String> = if resolved.eq_ignore_ascii_case(php_index::framework::raxos::router::CONTROLLER) {
        controllers
            .mounts(&class.name)
            .into_iter()
            .map(|mount| if mount.is_empty() { "/".to_string() } else { mount })
            .collect()
    } else {
        php_index::framework::raxos::router::route_verb(&resolved)?;
        let method = attribute
            .ancestors()
            .find(|node| node.kind() == METHOD_DECLARATION)
            .and_then(|method| ast::child_of(&method, NAME))
            .map(|name| ast::text_of(&name))?;
        info.routes
            .iter()
            .filter(|route| route.method.eq_ignore_ascii_case(&method))
            .flat_map(|route| controllers.route_paths(&class.name, route))
            .collect()
    };
    if paths.is_empty() {
        return None;
    }
    Some((format!("```\n{}\n```", paths.join("\n")), attribute.text_range()))
}

/// A `$name` of a Raxos route or controller path, with the function whose parameter of that name
/// the router puts there.
pub struct RouteSegment {
    pub name: String,
    /// The `$` and the name.
    pub range: php_syntax::TextRange,
    pub callee: crate::target::Callee,
}

/// The `$name` segments of a path literal of `#[Controller(prefix:)]` or a route attribute that
/// name a parameter: of the constructor for a prefix, of the method for a route.
pub fn route_segments(analyzer: &Analyzer<'_>, literal: &SyntaxNode) -> Vec<RouteSegment> {
    route_segments_of(analyzer, literal).unwrap_or_default()
}

fn route_segments_of(analyzer: &Analyzer<'_>, literal: &SyntaxNode) -> Option<Vec<RouteSegment>> {
    use php_index::framework::raxos::router::{CONTROLLER, path_parameters, route_verb};
    if !analyzer.index.frameworks().raxos || !literal.text().to_string().contains('$') {
        return None;
    }
    let argument = literal.parent().filter(|node| node.kind() == ARGUMENT)?;
    let list = argument.parent().filter(|node| node.kind() == ARGUMENT_LIST)?;
    let attribute = list.parent().filter(|node| node.kind() == ATTRIBUTE)?;
    let resolved = analyzer
        .resolver
        .resolve_class(&ast::text_of(&ast::child_of(&attribute, NAME)?));
    let (wanted, method) = if resolved.eq_ignore_ascii_case(CONTROLLER) {
        ("prefix", "__construct".to_string())
    } else {
        route_verb(&resolved)?;
        let method = attribute
            .ancestors()
            .find(|node| node.kind() == METHOD_DECLARATION)
            .and_then(|method| ast::child_of(&method, NAME))?;
        ("path", ast::text_of(&method))
    };
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
    let first = list.children().find(|child| child.kind() == ARGUMENT).as_ref() == Some(&argument);
    match named {
        Some(name) if name != wanted => return None,
        None if !first => return None,
        _ => {}
    }
    let class = analyzer.class.as_ref()?.name.clone();
    let params: Vec<String> = analyzer
        .index
        .class(&class)?
        .decl
        .method(&method)?
        .callable
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect();
    let (value, span) = php_index::test_facts::string_value(literal)?;
    Some(
        path_parameters(&value)
            .into_iter()
            .filter(|(_, name)| params.iter().any(|param| param == name))
            .map(|(at, name)| {
                let start = span.start + at as u32;
                RouteSegment {
                    name: name.to_string(),
                    range: ast::range_of(start, start + 1 + name.len() as u32),
                    callee: crate::target::Callee::Method {
                        class: class.clone(),
                        name: method.clone(),
                    },
                }
            })
            .collect(),
    )
}

/// The route segment under an offset.
pub fn route_segment_at(analyzer: &Analyzer<'_>, offset: u32) -> Option<RouteSegment> {
    let token = analyzer
        .root
        .token_at_offset(php_syntax::TextSize::from(offset))
        .find(|token| token.kind() == STRING_LITERAL)?;
    let literal = token.parent().filter(|node| node.kind() == LITERAL)?;
    route_segments(analyzer, &literal)
        .into_iter()
        .find(|segment| u32::from(segment.range.start()) <= offset && offset <= u32::from(segment.range.end()))
}

const ARRAY_LIST: &str = "Raxos\\Collection\\ArrayList";

/// What `column('buyer', 'id')` on a list of models gives: an `ArrayList` of what the last key holds,
/// each key read from what the one before it gave, the way `array_column` is applied in turn.
pub(crate) fn column_call_type(
    analyzer: &Analyzer<'_>,
    callees: &[crate::infer::ResolvedCallable],
    args: &[crate::infer::Arg],
) -> Option<php_index::Type> {
    use php_index::Type;
    if !analyzer.index.frameworks().raxos || args.is_empty() {
        return None;
    }
    let callee = callees.first()?;
    if !callee.name.rsplit("::").next()?.eq_ignore_ascii_case("column") {
        return None;
    }
    let receiver = callee.receiver.as_ref()?;
    let is_list = receiver.members().iter().any(|member| {
        matches!(member, Type::Class { name, .. }
            if analyzer.index.is_subclass_of(name, php_index::framework::raxos::orm::MODEL_ARRAY_LIST))
    });
    if !is_list {
        return None;
    }
    let mut holds = Type::class(model_of(analyzer.index, receiver)?);
    for arg in args {
        if arg.name.is_some() || arg.spread {
            return None;
        }
        let literal = arg.expr.as_ref().filter(|expr| expr.kind() == LITERAL)?;
        let (key, _) = php_index::test_facts::string_value(literal)?;
        let model = model_of(analyzer.index, &holds)?;
        let (_, property) = properties_of(analyzer.index, &model)
            .into_iter()
            .find(|(_, property)| property.answers_to(&key))?;
        holds = analyzer
            .index
            .find_property(&Type::class(model), &property.name)?
            .member
            .effective_type(analyzer.index.level)?
            .clone();
    }
    Some(Type::Class {
        name: ARRAY_LIST.to_string(),
        args: vec![Type::Int, holds],
    })
}
