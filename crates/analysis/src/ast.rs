//! Small helpers over the syntax tree that the analyses share.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};

pub fn tokens(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> {
    node.children_with_tokens().filter_map(SyntaxElement::into_token)
}

pub fn first_token(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    tokens(node).find(|token| token.kind() == kind)
}

pub fn has_token(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    first_token(node, kind).is_some()
}

pub fn child_of(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|child| child.kind() == kind)
}

pub fn text_of(node: &SyntaxNode) -> String {
    node.text().to_string()
}

pub fn start(node: &SyntaxNode) -> u32 {
    u32::from(node.text_range().start())
}

pub fn end(node: &SyntaxNode) -> u32 {
    u32::from(node.text_range().end())
}

pub fn range_of(start: u32, end: u32) -> TextRange {
    TextRange::new(TextSize::from(start), TextSize::from(end.max(start)))
}

/// The deepest node that covers an offset, which for an offset between two tokens is the one the
/// preceding token is in.
pub fn node_at(root: &SyntaxNode, offset: u32) -> SyntaxNode {
    let offset = offset.min(end(root));
    let range = range_of(offset, offset);
    match root.covering_element(range) {
        SyntaxElement::Node(node) => node,
        SyntaxElement::Token(token) => token.parent().unwrap_or_else(|| root.clone()),
    }
}

pub fn is_type_node(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        NAMED_TYPE | NULLABLE_TYPE | UNION_TYPE | INTERSECTION_TYPE | PAREN_TYPE
    )
}

pub fn is_function_like(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        FUNCTION_DECLARATION | METHOD_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | PROPERTY_HOOK
    )
}

/// The offset to ask for the variables that a node sees: the start of its statement, or a point
/// inside the arrow function or closure whose parameters it may use.
pub fn statement_anchor(node: &SyntaxNode) -> u32 {
    let mut current = node.clone();
    loop {
        let kind = current.kind();
        if matches!(kind, ARROW_FUNCTION_EXPR | CLOSURE_EXPR) && current != *node {
            return start(&current) + 1;
        }
        if matches!(
            kind,
            EXPR_STATEMENT
                | ECHO_STATEMENT
                | RETURN_STATEMENT
                | UNSET_STATEMENT
                | GLOBAL_STATEMENT
                | STATIC_VARIABLE_STATEMENT
        ) {
            return start(&current);
        }
        let Some(parent) = current.parent() else {
            return start(node);
        };
        if matches!(
            parent.kind(),
            BLOCK | STATEMENT_LIST | SOURCE_FILE | CASE_CLAUSE | DEFAULT_CLAUSE | CLASS_BODY
        ) {
            return start(&current);
        }
        current = parent;
    }
}

/// The function-like node a node sits in, or `None` at the top level of the file.
pub fn enclosing_function(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().find(|ancestor| is_function_like(ancestor.kind()))
}

pub fn is_class_like(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        CLASS_DECLARATION | INTERFACE_DECLARATION | TRAIT_DECLARATION | ENUM_DECLARATION | ANONYMOUS_CLASS
    )
}

/// The range of the last segment of a written name, which is the part a rename replaces.
pub fn last_segment(range: TextRange, text: &str) -> TextRange {
    let raw = &text[usize::from(range.start())..usize::from(range.end())];
    match raw.rfind('\\') {
        Some(at) => range_of(u32::from(range.start()) + at as u32 + 1, u32::from(range.end())),
        None => range,
    }
}

/// The range of a name token that a rename replaces: the last segment of a qualified name.
pub fn last_segment_of_token(token: &SyntaxToken) -> TextRange {
    let text = token.text();
    let range = token.text_range();
    match text.rfind('\\') {
        Some(at) => range_of(u32::from(range.start()) + at as u32 + 1, u32::from(range.end())),
        None => range,
    }
}
