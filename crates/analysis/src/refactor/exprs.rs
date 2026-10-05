//! Reading expressions and statements for refactors: which node a selection or a cursor means,
//! where a statement may be put before another, and what an expression does besides giving a value.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};

use crate::ast::{self, child_of, end, has_token, range_of, start, text_of, tokens};
use crate::refs::is_write_target;

pub(crate) fn is_expression(node: &SyntaxNode) -> bool {
    match node.kind() {
        LITERAL | VARIABLE_EXPR | VARIABLE_VARIABLE | ARRAY_EXPR | LIST_EXPR | PREFIX_EXPR | POSTFIX_EXPR
        | BINARY_EXPR | ASSIGN_EXPR | TERNARY_EXPR | CAST_EXPR | CLONE_EXPR | NEW_EXPR | PRINT_EXPR | EXIT_EXPR
        | ISSET_EXPR | EMPTY_EXPR | EVAL_EXPR | INCLUDE_EXPR | THROW_EXPR | YIELD_EXPR | YIELD_FROM_EXPR
        | MATCH_EXPR | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | PAREN_EXPR | CALL_EXPR | PROPERTY_FETCH_EXPR
        | SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR | INDEX_EXPR | SHELL_EXEC_EXPR | INTERPOLATED_STRING | HEREDOC => {
            true
        }
        NAME => is_value_name(node),
        _ => false,
    }
}

/// A name that stands for a constant: `PHP_EOL`, `true`, and not the name of a class or a member.
fn is_value_name(name: &SyntaxNode) -> bool {
    let Some(parent) = name.parent() else {
        return false;
    };
    let first = parent.children().next().as_ref() == Some(name);
    match parent.kind() {
        ARGUMENT | ARRAY_ITEM | RETURN_STATEMENT | ECHO_STATEMENT | PREFIX_EXPR | CAST_EXPR | MATCH_ARM
        | MATCH_EXPR | INDEX_EXPR | PAREN_EXPR | PRINT_EXPR | EXPR_STATEMENT | TERNARY_EXPR | THROW_EXPR
        | CLONE_EXPR | IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT | FOREACH_STATEMENT => true,
        ASSIGN_EXPR => !first,
        BINARY_EXPR => !has_token(&parent, INSTANCEOF_KW) || first,
        CONST_ELEMENT | PARAMETER | PROPERTY_ELEMENT => !first && !matches!(parent.kind(), PARAMETER),
        _ => false,
    }
}

pub(crate) fn trim_range(text: &str, range: TextRange) -> TextRange {
    let (mut from, mut to) = (usize::from(range.start()), usize::from(range.end()));
    while from < to && text[from..].starts_with(char::is_whitespace) {
        from += text[from..].chars().next().map_or(1, char::len_utf8);
    }
    while to > from && text[..to].ends_with(char::is_whitespace) {
        to -= text[..to].chars().next_back().map_or(1, char::len_utf8);
    }
    range_of(from as u32, to as u32)
}

/// The expression a selection spans exactly.
pub(crate) fn expression_of_selection(root: &SyntaxNode, text: &str, range: TextRange) -> Option<SyntaxNode> {
    let range = trim_range(text, range);
    if range.is_empty() {
        return None;
    }
    let covering = match root.covering_element(range) {
        SyntaxElement::Node(node) => node,
        SyntaxElement::Token(token) => token.parent()?,
    };
    covering
        .ancestors()
        .take_while(|node| node.text_range().contains_range(range))
        .filter(|node| node.text_range() == range && is_expression(node))
        .last()
}

/// The expressions around a position, innermost first, for as long as they are part of one
/// statement.
pub(crate) fn expressions_at(root: &SyntaxNode, offset: u32) -> Vec<SyntaxNode> {
    let Some(token) = token_near(root, offset) else {
        return Vec::new();
    };
    let mut out: Vec<SyntaxNode> = Vec::new();
    let mut current = token.parent();
    while let Some(node) = current {
        if is_expression(&node) {
            if node.kind() != NAME || out.is_empty() {
                out.push(node.clone());
            }
        } else if !matches!(
            node.kind(),
            ARGUMENT | ARGUMENT_LIST | ARRAY_ITEM | MATCH_ARM | BRACED_INTERPOLATION | NAME
        ) {
            break;
        }
        current = node.parent();
    }
    out
}

/// The token a position is on, preferring the one it starts over the one it ends.
pub(crate) fn token_near(root: &SyntaxNode, offset: u32) -> Option<SyntaxToken> {
    let usable = |token: &SyntaxToken| !token.kind().is_trivia();
    match root.token_at_offset(TextSize::from(offset.min(end(root)))) {
        php_syntax::TokenAtOffset::None => None,
        php_syntax::TokenAtOffset::Single(token) => usable(&token).then_some(token),
        php_syntax::TokenAtOffset::Between(left, right) => {
            if usable(&right) && !matches!(right.kind(), RPAREN | RBRACKET | RBRACE | SEMICOLON | COMMA) {
                Some(right)
            } else if usable(&left) {
                Some(left)
            } else if usable(&right) {
                Some(right)
            } else {
                None
            }
        }
    }
}

/// A node in the position of a statement of a list.
pub(crate) fn is_statement_position(node: &SyntaxNode) -> bool {
    node.parent().is_some_and(|parent| {
        matches!(
            parent.kind(),
            BLOCK | STATEMENT_LIST | SOURCE_FILE | CASE_CLAUSE | DEFAULT_CLAUSE
        )
    })
}

/// Whether an expression changes something or can be seen to run: a call, an assignment, `new`.
pub(crate) fn has_side_effects(node: &SyntaxNode) -> bool {
    node.descendants().any(|descendant| match descendant.kind() {
        CALL_EXPR | NEW_EXPR | ASSIGN_EXPR | INCLUDE_EXPR | YIELD_EXPR | YIELD_FROM_EXPR | PRINT_EXPR | EXIT_EXPR
        | EVAL_EXPR | SHELL_EXEC_EXPR | THROW_EXPR | CLONE_EXPR => true,
        PREFIX_EXPR | POSTFIX_EXPR => has_token(&descendant, INC) || has_token(&descendant, DEC),
        _ => false,
    })
}

/// An expression whose value does not depend on anything that can change: literals, constants,
/// and operators over them.
pub(crate) fn is_constant_like(node: &SyntaxNode) -> bool {
    node.descendants().all(|descendant| {
        matches!(
            descendant.kind(),
            LITERAL
                | NAME
                | BINARY_EXPR
                | PREFIX_EXPR
                | PAREN_EXPR
                | ARRAY_EXPR
                | ARRAY_ITEM
                | TERNARY_EXPR
                | SCOPED_ACCESS_EXPR
                | INTERPOLATED_STRING
        ) && !has_side_effects_kind(&descendant)
    })
}

fn has_side_effects_kind(node: &SyntaxNode) -> bool {
    matches!(node.kind(), PREFIX_EXPR) && (has_token(node, INC) || has_token(node, DEC))
}

/// The tokens of a node without the trivia, which is what makes two spellings of an expression equal.
pub(crate) fn token_text(node: &SyntaxNode) -> String {
    node.descendants_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .filter(|token| !token.kind().is_trivia())
        .map(|token| token.text().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The variables an expression reads, by name without the `$`, the ones inside functions it
/// creates included when they come from the outside.
pub(crate) fn variables_in(node: &SyntaxNode) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in node.descendants_with_tokens().filter_map(SyntaxElement::into_token) {
        if token.kind() != VARIABLE {
            continue;
        }
        let Some(parent) = token.parent() else {
            continue;
        };
        if !matches!(parent.kind(), VARIABLE_EXPR | CLOSURE_USE_VARIABLE) {
            continue;
        }
        if parent.kind() == VARIABLE_EXPR
            && parent.parent().is_some_and(|grand| {
                grand.kind() == STATIC_PROPERTY_EXPR && grand.children().next().as_ref() != Some(&parent)
            })
        {
            continue;
        }
        let name = token.text().trim_start_matches('$').to_string();
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Whether the node uses `$this`, `self`, `static` or `parent`, which only mean something inside a class.
pub(crate) fn uses_class_context(node: &SyntaxNode) -> bool {
    node.descendants_with_tokens().any(|element| match element {
        SyntaxElement::Token(token) => {
            (token.kind() == VARIABLE && token.text() == "$this")
                || (token.parent().is_some_and(|parent| parent.kind() == NAME)
                    && token.parent().and_then(|name| name.parent()).is_some_and(|owner| {
                        matches!(owner.kind(), SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR | NEW_EXPR)
                    })
                    && matches!(token.text().to_ascii_lowercase().as_str(), "self" | "static" | "parent"))
        }
        SyntaxElement::Node(_) => false,
    })
}

/// Why an expression cannot be moved out in front of its statement, or `Ok` with the statement.
pub(crate) struct HoistSite {
    pub statement: SyntaxNode,
    pub container: SyntaxNode,
}

/// Whether evaluating `expr` before its statement runs is the same as evaluating it where it is.
pub(crate) fn hoist_site(expr: &SyntaxNode) -> Result<HoistSite, String> {
    let mut child = expr.clone();
    let site = loop {
        let Some(parent) = child.parent() else {
            return Err("There is no statement around this expression".to_string());
        };
        match parent.kind() {
            IF_STATEMENT | ELSE_CLAUSE | ELSEIF_CLAUSE | WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT
            | FOREACH_STATEMENT
                if !is_expression(&child) && !matches!(child.kind(), BLOCK | STATEMENT_LIST) =>
            {
                return Err("Put braces around the body of the statement first".to_string());
            }
            ARROW_FUNCTION_EXPR => return Err("An arrow function has no statement to put a variable in".to_string()),
            ISSET_EXPR | EMPTY_EXPR | UNSET_STATEMENT => {
                return Err("An expression inside isset, empty or unset cannot be moved".to_string());
            }
            PREFIX_EXPR if has_token(&parent, AT) => return Err("The expression is silenced with @".to_string()),
            BINARY_EXPR => {
                let operator = binary_operator(&parent);
                let second = parent.children().nth(1).as_ref() == Some(&child);
                if matches!(operator, Some(AND_AND | OR_OR | AND_KW | OR_KW | COALESCE)) && second {
                    return Err("The expression only runs when the left side allows it".to_string());
                }
                if operator == Some(COALESCE) && !second && in_chain(expr, &child) {
                    return Err("The left side of ?? may be undefined, which a variable would report".to_string());
                }
            }
            ASSIGN_EXPR if has_token(&parent, COALESCE_ASSIGN) => {
                return Err("The expression only runs when the target is empty".to_string());
            }
            TERNARY_EXPR if parent.children().next().as_ref() != Some(&child) => {
                return Err("The expression only runs for one outcome of the condition".to_string());
            }
            MATCH_ARM => return Err("The expression only runs for one arm of the match".to_string()),
            MATCH_EXPR if parent.children().next().as_ref() != Some(&child) => {
                return Err("The expression only runs for one arm of the match".to_string());
            }
            CASE_CLAUSE if parent.children().next().as_ref() == Some(&child) => {
                return Err("A case is only evaluated when the ones above it did not match".to_string());
            }
            ELSEIF_CLAUSE if is_expression(&child) => {
                return Err("An elseif condition only runs when the ones above it were false".to_string());
            }
            WHILE_STATEMENT | DO_WHILE_STATEMENT if is_expression(&child) => {
                return Err("A loop condition runs on every pass".to_string());
            }
            FOR_STATEMENT if is_expression(&child) && !in_for_init(&parent, &child) => {
                return Err("A loop condition runs on every pass".to_string());
            }
            INTERPOLATED_STRING | HEREDOC
                if !matches!(child.kind(), BRACED_INTERPOLATION | DOLLAR_BRACE_INTERPOLATION) =>
            {
                return Err("A variable inside a string would change what the string says".to_string());
            }
            BRACED_INTERPOLATION | DOLLAR_BRACE_INTERPOLATION => {}
            ATTRIBUTE
            | PARAMETER
            | PROPERTY_ELEMENT
            | CONST_ELEMENT
            | STATIC_VARIABLE
            | ENUM_CASE
            | ATTRIBUTE_LIST
            | CLASS_CONST_DECLARATION
            | PROPERTY_DECLARATION
            | DECLARE_DIRECTIVE => {
                return Err("This place only takes constants".to_string());
            }
            _ => {}
        }
        if matches!(
            parent.kind(),
            BLOCK | STATEMENT_LIST | SOURCE_FILE | CASE_CLAUSE | DEFAULT_CLAUSE
        ) && !matches!(child.kind(), ERROR)
            && (is_statement_node(&child))
        {
            break HoistSite {
                statement: child,
                container: parent,
            };
        }
        child = parent;
    };
    if !matches!(
        site.statement.kind(),
        EXPR_STATEMENT
            | ECHO_STATEMENT
            | RETURN_STATEMENT
            | IF_STATEMENT
            | WHILE_STATEMENT
            | DO_WHILE_STATEMENT
            | FOR_STATEMENT
            | FOREACH_STATEMENT
            | SWITCH_STATEMENT
    ) {
        return Err("A variable cannot be put in front of this statement".to_string());
    }
    if conflicting_effects_before(expr, &site.statement) {
        return Err("Moving the expression out would change the order things run in".to_string());
    }
    Ok(site)
}

fn is_statement_node(node: &SyntaxNode) -> bool {
    !is_expression(node) || matches!(node.kind(), EXPR_STATEMENT)
}

fn in_for_init(statement: &SyntaxNode, child: &SyntaxNode) -> bool {
    let first_semicolon = tokens(statement).find(|token| token.kind() == SEMICOLON);
    first_semicolon.is_some_and(|semicolon| child.text_range().end() <= semicolon.text_range().start())
}

/// Whether the node reaches the one above through members and elements only, where an undefined
/// part is not reported.
fn in_chain(inner: &SyntaxNode, outer: &SyntaxNode) -> bool {
    let mut current = inner.clone();
    while current != *outer {
        let Some(parent) = current.parent() else {
            return false;
        };
        if !matches!(
            parent.kind(),
            INDEX_EXPR | PROPERTY_FETCH_EXPR | STATIC_PROPERTY_EXPR | SCOPED_ACCESS_EXPR
        ) {
            return false;
        }
        current = parent;
    }
    true
}

pub(crate) fn binary_operator(node: &SyntaxNode) -> Option<SyntaxKind> {
    tokens(node)
        .find(|token| !token.kind().is_trivia())
        .map(|token| token.kind())
}

/// Whether the expression only reads variables of the function, which only a call or an
/// assignment that names them can change.
fn is_local_pure(node: &SyntaxNode) -> bool {
    node.descendants().all(|descendant| match descendant.kind() {
        VARIABLE_EXPR => text_of(&descendant) != "$this",
        LITERAL | NAME | BINARY_EXPR | PAREN_EXPR | ARRAY_EXPR | ARRAY_ITEM | TERNARY_EXPR | INDEX_EXPR | CAST_EXPR
        | INTERPOLATED_STRING | SCOPED_ACCESS_EXPR => true,
        PREFIX_EXPR => !has_token(&descendant, INC) && !has_token(&descendant, DEC),
        _ => false,
    })
}

fn is_effect(node: &SyntaxNode) -> bool {
    match node.kind() {
        CALL_EXPR | NEW_EXPR | ASSIGN_EXPR | INCLUDE_EXPR | YIELD_EXPR | YIELD_FROM_EXPR | PRINT_EXPR | EXIT_EXPR
        | EVAL_EXPR | SHELL_EXEC_EXPR | CLONE_EXPR => true,
        PREFIX_EXPR | POSTFIX_EXPR => has_token(node, INC) || has_token(node, DEC),
        _ => false,
    }
}

/// Whether something evaluated in the statement before the expression can change what it gives.
fn conflicting_effects_before(expr: &SyntaxNode, statement: &SyntaxNode) -> bool {
    if is_constant_like(expr) {
        return false;
    }
    let from = start(expr);
    let local = is_local_pure(expr);
    let reads = variables_in(expr);
    statement.descendants().any(|node| {
        if node == *statement || end(&node) > from || node.ancestors().any(|ancestor| ancestor == *expr) {
            return false;
        }
        is_effect(&node) && (!local || variables_in(&node).iter().any(|name| reads.contains(name)))
    })
}

/// Where a statement is put in front of another: at the start of its line when it has the line to
/// itself, else right before it.
pub(crate) struct Insertion {
    pub offset: u32,
    pub text: String,
}

pub(crate) fn insertion_before(text: &str, statement: &SyntaxNode, code: &str) -> Insertion {
    let at = start(statement) as usize;
    let line = crate::actions::edits::line_start(text, at);
    let prefix = &text[line..at];
    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if prefix.trim().is_empty() {
        Insertion {
            offset: line as u32,
            text: format!("{prefix}{code}{eol}"),
        }
    } else {
        Insertion {
            offset: at as u32,
            text: format!("{code} "),
        }
    }
}

pub(crate) fn text_slice<'a>(text: &'a str, node: &SyntaxNode) -> &'a str {
    &text[start(node) as usize..end(node) as usize]
}

/// The function-like or file node a node belongs to.
pub(crate) fn scope_of(node: &SyntaxNode) -> SyntaxNode {
    ast::enclosing_function(node).unwrap_or_else(|| node.ancestors().last().unwrap_or_else(|| node.clone()))
}

pub(crate) fn name_of(node: &SyntaxNode) -> Option<String> {
    child_of(node, NAME).map(|name| text_of(&name))
}

/// Whether something else can stand where an expression is, now or as a call: not where it is
/// written to or called, where only a variable fits, or where an undefined part is not reported.
pub(crate) fn replaceable(expr: &SyntaxNode) -> Result<(), String> {
    if is_write_target(expr) {
        return Err("The expression is written to".to_string());
    }
    if matches!(expr.kind(), THROW_EXPR | EXIT_EXPR | YIELD_EXPR | YIELD_FROM_EXPR) {
        return Err("The expression leaves or hands back, which a value cannot".to_string());
    }
    if let Some(parent) = expr.parent() {
        if expr.kind() == VARIABLE_EXPR
            && parent.kind() == STATIC_PROPERTY_EXPR
            && parent.children().next().as_ref() != Some(expr)
        {
            return Err("The expression is the name of a static property".to_string());
        }
    }
    let mut child = expr.clone();
    while let Some(parent) = child.parent() {
        let first = parent.children().next().as_ref() == Some(&child);
        match parent.kind() {
            CALL_EXPR if first && child == *expr => return Err("The expression is what is called".to_string()),
            PREFIX_EXPR if has_token(&parent, AMP) => return Err("The expression is taken by reference".to_string()),
            PREFIX_EXPR if has_token(&parent, AT) => return Err("The expression is silenced with @".to_string()),
            ARGUMENT if has_token(&parent, ELLIPSIS) => return Err("The expression is spread".to_string()),
            ARRAY_ITEM if has_token(&parent, AMP) || has_token(&parent, ELLIPSIS) => {
                return Err("The expression is taken by reference".to_string());
            }
            ISSET_EXPR | EMPTY_EXPR | UNSET_STATEMENT => {
                return Err("An expression inside isset, empty or unset cannot be moved".to_string());
            }
            BINARY_EXPR if binary_operator(&parent) == Some(COALESCE) && first && in_chain(expr, &child) => {
                return Err("The left side of ?? may be undefined, which a variable would report".to_string());
            }
            ASSIGN_EXPR if has_token(&parent, COALESCE_ASSIGN) && first && in_chain(expr, &child) => {
                return Err("The target of ??= may be undefined".to_string());
            }
            INTERPOLATED_STRING | HEREDOC
                if !matches!(child.kind(), BRACED_INTERPOLATION | DOLLAR_BRACE_INTERPOLATION) =>
            {
                return Err("The expression is part of a string".to_string());
            }
            FOREACH_STATEMENT | ARROW_FUNCTION_EXPR | CLOSURE_EXPR => break,
            ATTRIBUTE
            | CONST_ELEMENT
            | PROPERTY_ELEMENT
            | STATIC_VARIABLE
            | ENUM_CASE
            | DECLARE_DIRECTIVE
            | CLASS_CONST_DECLARATION
            | PROPERTY_DECLARATION => {
                return Err("This place only takes constants".to_string());
            }
            PARAMETER => return Err("This place only takes constants".to_string()),
            BLOCK | STATEMENT_LIST | SOURCE_FILE => break,
            _ => {}
        }
        child = parent;
    }
    Ok(())
}

/// Takes a member out with its lines, and the blank line that set it apart from the next one when
/// it was the first.
pub(crate) fn remove_member(text: &str, range: TextRange) -> crate::completion::TextEdit {
    let mut edit = crate::actions::edits::remove_with_lines(text, range);
    let before = text[..edit.start as usize].trim_end_matches([' ', '\t']);
    let opens_body = before.ends_with("{\n") || before.ends_with("{\r\n");
    let rest = &text[edit.end as usize..];
    if opens_body {
        if rest.starts_with("\r\n") {
            edit.end += 2;
        } else if rest.starts_with('\n') {
            edit.end += 1;
        }
    }
    edit
}
