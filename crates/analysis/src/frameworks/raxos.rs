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
