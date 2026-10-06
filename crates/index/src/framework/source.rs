//! Reading the bodies and literals of a file the declarations do not keep: what a method returns,
//! what an array holds.

use std::path::Path;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, parse};

use crate::extract::resolver_at;
use crate::index::Index;
use crate::model::Span;
use crate::resolve::NameResolver;
use crate::test_facts::{class_constant, string_value};
use crate::types::Name;

/// The tree of a file as a person sees it, read when asked for.
pub fn tree_of(index: &Index, path: &Path) -> Option<SyntaxNode> {
    let text = index.read_text(path)?;
    Some(parse(&text).syntax())
}

/// The method declaration whose name starts at an offset.
pub fn method_at(root: &SyntaxNode, name_start: u32) -> Option<SyntaxNode> {
    let token = root.token_at_offset(php_syntax::TextSize::from(name_start));
    let token = match token {
        php_syntax::TokenAtOffset::Single(token) => token,
        php_syntax::TokenAtOffset::Between(_, right) => right,
        php_syntax::TokenAtOffset::None => return None,
    };
    token.parent_ancestors().find(|node| node.kind() == METHOD_DECLARATION)
}

/// The expression of the first `return` of a function-like node, not looking inside nested
/// functions.
pub fn returned_expression(function: &SyntaxNode) -> Option<SyntaxNode> {
    let body = function.children().find(|child| child.kind() == BLOCK)?;
    for node in body.descendants() {
        if node.kind() != RETURN_STATEMENT {
            continue;
        }
        let nested = node
            .ancestors()
            .skip(1)
            .take_while(|ancestor| ancestor != &body)
            .any(|ancestor| {
                matches!(
                    ancestor.kind(),
                    CLOSURE_EXPR | ARROW_FUNCTION_EXPR | FUNCTION_DECLARATION
                )
            });
        if !nested {
            return node.children().next();
        }
    }
    None
}

/// What a literal expression holds, when it is a string or `Foo::class`.
pub enum Literal {
    Text(String, Span),
    Class(Name),
}

pub fn literal_of(node: &SyntaxNode) -> Option<Literal> {
    if let Some((text, span)) = string_value(node) {
        return Some(Literal::Text(text, span));
    }
    let resolver = resolver_for(node);
    class_constant(node, &resolver).map(Literal::Class)
}

/// The names in effect where a node is written.
pub fn resolver_for(node: &SyntaxNode) -> NameResolver {
    let root = node.ancestors().last().unwrap_or_else(|| node.clone());
    resolver_at(&root, u32::from(node.text_range().start()))
}

/// The items of an array written out, with their keys when they have one. `None` when a spread
/// makes the contents unknown.
pub fn array_items(array: &SyntaxNode) -> Option<Vec<(Option<SyntaxNode>, SyntaxNode)>> {
    if array.kind() != ARRAY_EXPR {
        return None;
    }
    let mut out = Vec::new();
    for item in array.children().filter(|child| child.kind() == ARRAY_ITEM) {
        let has_spread = item.children_with_tokens().any(|element| element.kind() == ELLIPSIS);
        if has_spread {
            return None;
        }
        let parts: Vec<SyntaxNode> = item.children().collect();
        match parts.as_slice() {
            [key, value] => out.push((Some(key.clone()), value.clone())),
            [value] => out.push((None, value.clone())),
            _ => return None,
        }
    }
    Some(out)
}

pub fn span_of(node: &SyntaxNode) -> Span {
    let range = node.text_range();
    Span {
        start: u32::from(range.start()),
        end: u32::from(range.end()),
    }
}

/// One call of a chain such as `$table->string('name')->nullable()`.
pub struct Link {
    pub name: String,
    pub args: Vec<SyntaxNode>,
    pub node: SyntaxNode,
}

/// The variable a chain of calls starts at and the calls in order. `None` when the expression is
/// not such a chain.
pub fn variable_chain(expression: &SyntaxNode) -> Option<(String, Vec<Link>)> {
    let mut links = Vec::new();
    let mut current = expression.clone();
    loop {
        match current.kind() {
            CALL_EXPR => {
                let callee = current.children().next()?;
                if callee.kind() != PROPERTY_FETCH_EXPR {
                    return None;
                }
                let name = callee.children().find(|child| child.kind() == NAME)?;
                links.push(Link {
                    name: name.text().to_string(),
                    args: crate::test_facts::argument_expressions(&current),
                    node: current.clone(),
                });
                current = callee.children().next()?;
            }
            VARIABLE_EXPR => {
                links.reverse();
                return Some((current.text().to_string().trim_start_matches('$').to_string(), links));
            }
            _ => return None,
        }
    }
}

/// The name of the class a static call is made on, as written.
pub fn static_call_parts(call: &SyntaxNode) -> Option<(String, String, SyntaxNode)> {
    if call.kind() != CALL_EXPR {
        return None;
    }
    let callee = call.children().next()?;
    if callee.kind() != SCOPED_ACCESS_EXPR {
        return None;
    }
    let mut names = callee.children().filter(|child| child.kind() == NAME);
    let class = names.next()?;
    let method = names.next()?;
    Some((class.text().to_string(), method.text().to_string(), call.clone()))
}

/// The class `Foo::class` names where an attribute argument of a class is written, which only the
/// file's imports can say.
pub fn class_in_attribute(index: &Index, class: crate::index::Class<'_>, text: &str) -> Option<crate::types::Name> {
    let name = text.trim().strip_suffix("::class")?.trim();
    let tree = tree_of(index, &class.file.path)?;
    let resolver = resolver_at(&tree, class.decl.span.start);
    Some(resolver.resolve_class(name))
}

/// The class and method `Foo::bar(...)` names where an attribute argument holds a first-class
/// callable, with the class resolved by the file's imports.
pub fn method_in_attribute(
    index: &Index,
    class: crate::index::Class<'_>,
    text: &str,
) -> Option<(crate::types::Name, String)> {
    let (name, rest) = text.trim().strip_suffix("(...)")?.split_once("::")?;
    let tree = tree_of(index, &class.file.path)?;
    let resolver = resolver_at(&tree, class.decl.span.start);
    Some((resolver.resolve_class(name.trim()), rest.trim().to_string()))
}
