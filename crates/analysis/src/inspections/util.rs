//! Small helpers over the tree that the inspections share.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode};

use crate::ast::{self, child_of, has_token, text_of};
use crate::refs::is_write_target;

/// The names a type position holds that are not classes.
pub fn is_builtin_type(lower: &str) -> bool {
    matches!(
        lower,
        "int"
            | "float"
            | "string"
            | "bool"
            | "array"
            | "callable"
            | "iterable"
            | "object"
            | "mixed"
            | "void"
            | "null"
            | "never"
            | "false"
            | "true"
            | "self"
            | "static"
            | "parent"
    )
}

/// `self`, `static` and `parent`, which name the class around them.
pub fn is_relative_class(lower: &str) -> bool {
    matches!(lower, "self" | "static" | "parent")
}

/// The member name of `$a->name`, `A::name` or `A::$name`, or `None` for a dynamic name.
pub fn member_name(access: &SyntaxNode) -> Option<SyntaxNode> {
    let first = access.children().next()?;
    access
        .children()
        .filter(|child| child.kind() == NAME)
        .last()
        .filter(|name| *name != first)
}

/// Whether a node is what a call calls.
pub fn is_callee(node: &SyntaxNode) -> bool {
    node.parent()
        .is_some_and(|parent| parent.kind() == CALL_EXPR && parent.children().next().as_ref() == Some(node))
}

/// Whether a node sits where a missing value is the point: an `isset`, an `empty`, or the left of `??`.
pub fn in_guard(node: &SyntaxNode) -> bool {
    let mut current = node.clone();
    while let Some(parent) = current.parent() {
        match parent.kind() {
            ISSET_EXPR | EMPTY_EXPR => return true,
            BINARY_EXPR => {
                let is_coalesce = ast::tokens(&parent).any(|token| token.kind() == COALESCE);
                if is_coalesce && parent.children().next().as_ref() == Some(&current) {
                    return true;
                }
            }
            ASSIGN_EXPR => {
                let is_coalesce = ast::tokens(&parent).any(|token| token.kind() == COALESCE_ASSIGN);
                if is_coalesce && parent.children().next().as_ref() == Some(&current) {
                    return true;
                }
                return false;
            }
            kind if is_boundary(kind) => return false,
            _ => {}
        }
        current = parent;
    }
    false
}

fn is_boundary(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        EXPR_STATEMENT
            | ECHO_STATEMENT
            | RETURN_STATEMENT
            | BLOCK
            | STATEMENT_LIST
            | SOURCE_FILE
            | ARGUMENT_LIST
            | CLOSURE_EXPR
            | ARROW_FUNCTION_EXPR
    )
}

/// Whether a condition around a node asks about a name first, with a function such as
/// `function_exists` or `property_exists`: code behind it is written for the name being absent.
pub fn guarded_by_check(node: &SyntaxNode, checks: &[&str], word: &str) -> bool {
    let word = word.to_ascii_lowercase();
    for ancestor in node.ancestors() {
        let condition = match ancestor.kind() {
            IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT => ancestor
                .children()
                .find(|child| !matches!(child.kind(), BLOCK | STATEMENT_LIST | ELSEIF_CLAUSE | ELSE_CLAUSE)),
            TERNARY_EXPR => ancestor.children().next(),
            BINARY_EXPR => ancestor.children().next(),
            _ => None,
        };
        let Some(condition) = condition else {
            continue;
        };
        if condition.text_range().contains_range(node.text_range()) {
            continue;
        }
        let text = condition.text().to_string().to_ascii_lowercase();
        if checks.iter().any(|check| text.contains(check)) && text.contains(&word) {
            return true;
        }
    }
    false
}

/// The variable at the root of `$a`, `$a->b`, `$a['x']` or `$a->b()`.
pub fn root_variable(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut current = node.clone();
    loop {
        match current.kind() {
            VARIABLE_EXPR => return Some(current),
            PROPERTY_FETCH_EXPR | INDEX_EXPR | CALL_EXPR | PAREN_EXPR | POSTFIX_EXPR => {
                current = current.children().next()?;
            }
            SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR => return None,
            _ => return None,
        }
    }
}

/// Whether the variable at the root of an expression is assigned somewhere inside a loop that
/// holds the expression. The type layer does not follow a loop around, so what it says of such a
/// variable is only what it was before the loop.
pub fn reassigned_in_loop(node: &SyntaxNode) -> bool {
    let Some(variable) = root_variable(node) else {
        return false;
    };
    let name = text_of(&variable);
    if name == "$this" {
        return false;
    }
    for ancestor in variable.ancestors() {
        match ancestor.kind() {
            WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT => {
                if assigns_variable(&ancestor, &name) {
                    return true;
                }
            }
            FUNCTION_DECLARATION | METHOD_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR => return false,
            _ => {}
        }
    }
    false
}

/// Whether a variable is written anywhere in a node, not counting the functions inside it.
pub fn assigns_variable(node: &SyntaxNode, name: &str) -> bool {
    let mut preorder = node.preorder();
    while let Some(event) = preorder.next() {
        let php_syntax::WalkEvent::Enter(current) = event else {
            continue;
        };
        if current != *node && matches!(current.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION | CLOSURE_EXPR) {
            preorder.skip_subtree();
            continue;
        }
        if current.kind() == VARIABLE_EXPR && text_of(&current) == name && is_write_target(&current) {
            return true;
        }
        let by_reference =
            current.kind() == CLOSURE_USE_VARIABLE && ast::tokens(&current).any(|token| token.kind() == AMP);
        if by_reference && ast::first_token(&current, VARIABLE).is_some_and(|token| token.text() == name) {
            return true;
        }
    }
    false
}

/// Whether the variable passed by reference could be changed by a call: an argument that is a
/// bare variable.
pub fn is_bare_variable(node: &SyntaxNode) -> bool {
    node.kind() == VARIABLE_EXPR
}

/// Whether a method declaration has a modifier.
pub fn has_modifier(declaration: &SyntaxNode, kind: SyntaxKind) -> bool {
    child_of(declaration, MODIFIER_LIST).is_some_and(|list| has_token(&list, kind))
}
