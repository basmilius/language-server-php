//! Extract method: a run of statements or one expression becomes a method, a function when it is
//! cut from one. What the code reads from around it becomes the parameters, what it leaves
//! behind that is read afterwards is returned, and the place it came from calls it.

use php_index::{Type, Visibility};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode, SyntaxToken, TextRange};

use super::draft::{Draft, focus};
use super::exprs::{expression_of_selection, is_statement_position, text_slice, trim_range};
use super::names::{camel, variable_name};
use super::signature::{parameter_name, parameters};
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::actions::edits::{indent_of, insert, replace};
use crate::actions::imports::ClassWriter;
use crate::actions::type_text::native_type_text;
use crate::ast::{self, child_of, end, first_token, has_token, start, text_of};
use crate::inspections::flow::{statements_complete, statements_leave};
use crate::inspections::types::sure_type;
use crate::refs::{Access, Hit, HitKind, binding_scope, variable_hits};

const SUPERGLOBALS: &[&str] = &[
    "this", "GLOBALS", "_SERVER", "_GET", "_POST", "_FILES", "_COOKIE", "_SESSION", "_REQUEST", "_ENV",
];

/// What is extracted.
enum Piece {
    Statements(Vec<SyntaxNode>),
    Expression(SyntaxNode),
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    if rcx.range.is_empty() {
        return;
    }
    if piece_of(rcx).is_none() {
        return;
    }
    let range = rcx.range;
    out.push(Refactor::new(
        format!("extract-method@{}-{}", u32::from(range.start()), u32::from(range.end())),
        "Extract method",
        RefactorKind::Extract,
        true,
        move || extract(rcx),
    ));
}

/// The statements or the expression a selection spans, and the function around them.
fn piece_of(rcx: &Rcx<'_>) -> Option<(Piece, SyntaxNode)> {
    let cx = &rcx.cx;
    let range = trim_range(cx.text, rcx.range);
    if range.is_empty() {
        return None;
    }
    if let Some(expr) = expression_of_selection(&cx.root, cx.text, range) {
        let function = ast::enclosing_function(&expr)?;
        let in_body =
            child_of(&function, BLOCK).is_some_and(|body| body.text_range().contains_range(expr.text_range()));
        if !in_body || super::exprs::replaceable(&expr).is_err() {
            return None;
        }
        return Some((Piece::Expression(expr), function));
    }
    let covering = match cx.root.covering_element(range) {
        SyntaxElement::Node(node) => node,
        SyntaxElement::Token(token) => token.parent()?,
    };
    let container = covering
        .ancestors()
        .find(|node| matches!(node.kind(), BLOCK | STATEMENT_LIST | CASE_CLAUSE | DEFAULT_CLAUSE))?;
    let statements: Vec<SyntaxNode> = container
        .children()
        .filter(|node| is_statement_position(node) && range.contains_range(node.text_range()))
        .collect();
    let (first, last) = (statements.first()?, statements.last()?);
    let gap_is_blank = |from: u32, to: u32| {
        let slice = &cx.text[from as usize..to as usize];
        let mut rest = slice;
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                return true;
            }
            if let Some(after) = rest.strip_prefix("//").or_else(|| rest.strip_prefix('#')) {
                rest = after.find('\n').map_or("", |at| &after[at..]);
            } else if let Some(after) = rest.strip_prefix("/*") {
                match after.find("*/") {
                    Some(at) => rest = &after[at + 2..],
                    None => return false,
                }
            } else {
                return false;
            }
        }
    };
    if !gap_is_blank(u32::from(range.start()), start(first)) || !gap_is_blank(end(last), u32::from(range.end())) {
        return None;
    }
    for pair in statements.windows(2) {
        if !gap_is_blank(end(&pair[0]), start(&pair[1])) {
            return None;
        }
    }
    let function = ast::enclosing_function(first)?;
    Some((Piece::Statements(statements), function))
}

/// What a variable does in the extracted code.
struct Flow {
    name: String,
    input: bool,
    output: bool,
    by_reference: bool,
    /// The function's own parameter, as nothing has changed it before the code: what it declares is the truth.
    untouched_parameter: bool,
}

fn is_superglobal(name: &str) -> bool {
    SUPERGLOBALS.contains(&name)
}

fn variable_tokens(piece_nodes: &[SyntaxNode]) -> Vec<SyntaxToken> {
    let mut out = Vec::new();
    for node in piece_nodes {
        for element in node.descendants_with_tokens() {
            if let SyntaxElement::Token(token) = element {
                if token.kind() == VARIABLE
                    && token
                        .parent()
                        .is_some_and(|parent| matches!(parent.kind(), VARIABLE_EXPR | CLOSURE_USE_VARIABLE))
                {
                    out.push(token);
                }
            }
        }
    }
    out
}

fn nodes_of(piece: &Piece) -> Vec<SyntaxNode> {
    match piece {
        Piece::Statements(list) => list.clone(),
        Piece::Expression(expr) => vec![expr.clone()],
    }
}

fn span_of(piece: &Piece) -> (u32, u32) {
    match piece {
        Piece::Statements(list) => (list.first().map_or(0, start), list.last().map_or(0, end)),
        Piece::Expression(expr) => (start(expr), end(expr)),
    }
}

/// Everything in the way of cutting the code out, found before any edit is written.
fn check_cuttable(piece: &Piece) -> Result<(), String> {
    for node in nodes_of(piece) {
        for descendant in node.descendants() {
            match descendant.kind() {
                GOTO_STATEMENT | LABEL_STATEMENT => return Err("The selection jumps with goto".to_string()),
                FUNCTION_DECLARATION
                | CLASS_DECLARATION
                | INTERFACE_DECLARATION
                | TRAIT_DECLARATION
                | ENUM_DECLARATION
                | USE_STATEMENT
                | CONST_STATEMENT
                | NAMESPACE_DECLARATION
                | DECLARE_STATEMENT => {
                    return Err("The selection declares something that has to stay where it is".to_string());
                }
                STATIC_VARIABLE_STATEMENT | GLOBAL_STATEMENT => {
                    return Err("The selection binds variables that outlive a call".to_string());
                }
                VARIABLE_VARIABLE | EVAL_EXPR | INCLUDE_EXPR => {
                    return Err("The selection reads variables by name".to_string());
                }
                CALL_EXPR => {
                    let callee = descendant.children().next();
                    if let Some(callee) = callee.filter(|callee| callee.kind() == NAME) {
                        let called = text_of(&callee).trim_start_matches('\\').to_ascii_lowercase();
                        if matches!(
                            called.as_str(),
                            "compact"
                                | "extract"
                                | "get_defined_vars"
                                | "func_get_args"
                                | "func_get_arg"
                                | "func_num_args"
                        ) {
                            return Err("The selection reads variables by name".to_string());
                        }
                    }
                }
                CLOSURE_USE_VARIABLE if has_token(&descendant, AMP) => {
                    return Err("The selection binds a variable by reference".to_string());
                }
                ASSIGN_EXPR if assigns_reference(&descendant) => {
                    return Err("The selection binds a reference".to_string());
                }
                PREFIX_EXPR | ARRAY_ITEM | ARGUMENT | PARAMETER
                    if descendant.kind() != PARAMETER && by_ref_marker(&descendant) =>
                {
                    return Err("The selection binds a reference".to_string());
                }
                FOREACH_STATEMENT if foreach_by_reference(&descendant) => {
                    return Err("The selection binds a reference".to_string());
                }
                BREAK_STATEMENT | CONTINUE_STATEMENT if jumps_out(&descendant, piece) => {
                    return Err("The selection leaves a loop that it does not contain".to_string());
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// `$a = &$b`.
fn assigns_reference(assign: &SyntaxNode) -> bool {
    let mut after_assign = false;
    for token in ast::tokens(assign) {
        if token.kind() == ASSIGN {
            after_assign = true;
        } else if after_assign && !token.kind().is_trivia() {
            return token.kind() == AMP;
        }
    }
    false
}

/// `&$x` inside an array literal.
fn by_ref_marker(node: &SyntaxNode) -> bool {
    match node.kind() {
        ARRAY_ITEM => has_token(node, AMP),
        PREFIX_EXPR => false,
        _ => false,
    }
}

fn foreach_by_reference(statement: &SyntaxNode) -> bool {
    ast::tokens(statement).any(|token| token.kind() == AMP)
}

/// A `break` or `continue` that reaches a loop or switch outside the selection.
fn jumps_out(jump: &SyntaxNode, piece: &Piece) -> bool {
    let levels: usize = ast::tokens(jump)
        .find(|token| token.kind() == INT_LITERAL)
        .and_then(|token| token.text().parse().ok())
        .or_else(|| {
            jump.descendants_with_tokens()
                .filter_map(SyntaxElement::into_token)
                .find(|token| token.kind() == INT_LITERAL)
                .and_then(|token| token.text().parse().ok())
        })
        .unwrap_or(1);
    let (from, to) = span_of(piece);
    let inside = jump
        .ancestors()
        .skip(1)
        .take_while(|node| start(node) >= from && end(node) <= to)
        .filter(|node| {
            matches!(
                node.kind(),
                WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT | SWITCH_STATEMENT
            )
        })
        .count();
    levels > inside
}

/// A node of the selection that belongs to the function being cut, not to a closure inside it.
fn in_own_scope(node: &SyntaxNode, function: &SyntaxNode) -> bool {
    ast::enclosing_function(node).as_ref() == Some(function)
}

fn own_nodes(piece: &Piece, function: &SyntaxNode, kinds: &[php_syntax::SyntaxKind]) -> Vec<SyntaxNode> {
    nodes_of(piece)
        .iter()
        .flat_map(|node| node.descendants())
        .filter(|node| kinds.contains(&node.kind()) && in_own_scope(node, function))
        .collect()
}

/// The variables the code uses, in the order it first uses them, and what it does with each.
fn flows(rcx: &Rcx<'_>, piece: &Piece, function: &SyntaxNode) -> Result<Vec<Flow>, String> {
    let cx = &rcx.cx;
    let nodes = nodes_of(piece);
    let (from, to) = span_of(piece);
    let mut names: Vec<String> = Vec::new();
    for token in variable_tokens(&nodes) {
        let name = token.text().trim_start_matches('$').to_string();
        if is_superglobal(&name) || names.contains(&name) {
            continue;
        }
        let scope = binding_scope(&cx.root, &token, &name);
        if scope == *function {
            names.push(name);
        }
    }
    let loop_around = nodes.first().and_then(|node| {
        node.ancestors()
            .filter(|ancestor| {
                matches!(
                    ancestor.kind(),
                    WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT
                ) && !ancestor.text_range().contains_range(function.text_range())
                    && function.text_range().contains_range(ancestor.text_range())
            })
            .last()
    });
    let bound = bound_names(function);
    if let Some(name) = names.iter().find(|name| bound.contains(name)) {
        return Err(format!("The selection uses ${name}, which `global` or `static` binds"));
    }
    let by_reference_params = by_reference_parameters(function);
    let taken_by_calls = super::scope::by_reference_variables(cx, function);
    let mut out = Vec::new();
    for name in names {
        let mut hits = variable_hits(&cx.file, (start(function), end(function)), &name);
        for hit in &mut hits {
            if taken_by_calls.contains(&u32::from(hit.range.start())) {
                hit.access = Access::Write;
            }
        }
        let hits: Vec<&Hit> = hits.iter().filter(|hit| hit.kind != HitKind::Doc).collect();
        let inside: Vec<&&Hit> = hits
            .iter()
            .filter(|hit| u32::from(hit.range.start()) >= from && u32::from(hit.range.end()) <= to)
            .collect();
        let declared_before = hits.iter().any(|hit| {
            u32::from(hit.range.end()) <= from && (hit.kind == HitKind::Declaration || hit.access == Access::Write)
        });
        let input = declared_before && reads_before_writing(&cx.root, &inside);
        let written = inside.iter().any(|hit| hit.access == Access::Write);
        let live_after = hits
            .iter()
            .any(|hit| needs_previous(&cx.root, hit) && u32::from(hit.range.start()) >= to)
            || loop_around.as_ref().is_some_and(|looping| {
                hits.iter().any(|hit| {
                    needs_previous(&cx.root, hit)
                        && u32::from(hit.range.start()) < from
                        && looping.text_range().contains_range(hit.range)
                })
            });
        let by_reference = by_reference_params.contains(&name) && written;
        let is_parameter = hits.iter().any(|hit| hit.kind == HitKind::Declaration);
        let changed_before = hits.iter().any(|hit| {
            hit.kind != HitKind::Declaration && hit.access == Access::Write && u32::from(hit.range.end()) <= from
        });
        out.push(Flow {
            untouched_parameter: is_parameter && !changed_before,
            name,
            input: input || by_reference,
            output: written && live_after && !by_reference,
            by_reference,
        });
    }
    Ok(out)
}

/// Whether a place needs the value the variable had: a read, or an update such as `++` or `.=`.
fn needs_previous(root: &SyntaxNode, hit: &Hit) -> bool {
    hit.access == Access::Read || is_update(root, hit)
}

/// A write that builds on what the variable held: `++`, `.=`, `[] =`, `unset`.
fn is_update(root: &SyntaxNode, hit: &Hit) -> bool {
    if hit.access != Access::Write || hit.kind == HitKind::Declaration {
        return false;
    }
    let Some(token) = root.covering_element(hit.range).into_token() else {
        return false;
    };
    let Some(mut node) = token.parent() else {
        return false;
    };
    if node.kind() != VARIABLE_EXPR {
        return false;
    }
    loop {
        let Some(parent) = node.parent() else {
            return false;
        };
        let first = parent.children().next().as_ref() == Some(&node);
        match parent.kind() {
            INDEX_EXPR if first => return true,
            PREFIX_EXPR | POSTFIX_EXPR => return has_token(&parent, INC) || has_token(&parent, DEC),
            UNSET_STATEMENT => return true,
            ASSIGN_EXPR => return first && first_token(&parent, ASSIGN).is_none(),
            ARRAY_ITEM | ARRAY_EXPR | LIST_EXPR => node = parent,
            _ => return false,
        }
    }
}

/// The variables a function ties to something that outlives a call with `global` or `static`.
fn bound_names(function: &SyntaxNode) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for node in function.descendants() {
        if !matches!(node.kind(), GLOBAL_STATEMENT | STATIC_VARIABLE_STATEMENT)
            || ast::enclosing_function(&node).as_ref() != Some(function)
        {
            continue;
        }
        for token in node.descendants_with_tokens().filter_map(SyntaxElement::into_token) {
            if token.kind() == VARIABLE {
                out.push(token.text().trim_start_matches('$').to_string());
            }
        }
    }
    out
}

fn by_reference_parameters(function: &SyntaxNode) -> Vec<String> {
    let Some(list) = child_of(function, PARAMETER_LIST) else {
        return Vec::new();
    };
    list.children()
        .filter(|parameter| parameter.kind() == PARAMETER && has_token(parameter, AMP))
        .filter_map(|parameter| first_token(&parameter, VARIABLE))
        .map(|token| token.text().trim_start_matches('$').to_string())
        .collect()
}

/// Whether something in the code needs the value a variable has before it, which is a read or an
/// update that no assignment of the code itself comes before.
fn reads_before_writing(root: &SyntaxNode, hits: &[&&Hit]) -> bool {
    let mut ordered: Vec<(u32, &Hit)> = hits
        .iter()
        .map(|hit| {
            let hit: &Hit = hit;
            (evaluated_at(root, hit), hit)
        })
        .collect();
    ordered.sort_by_key(|(at, _)| *at);
    let mut fresh: Vec<TextRange> = Vec::new();
    for (_, hit) in ordered {
        let covered = fresh.iter().any(|scope| scope.contains_range(hit.range));
        if hit.access == Access::Write && !is_update(root, hit) {
            if let Some(scope) = fresh_write_scope(root, hit) {
                fresh.push(scope);
            }
        } else if !covered {
            return true;
        }
    }
    false
}

/// Where in the order of evaluation a hit sits: a plain assignment takes effect after its value.
fn evaluated_at(root: &SyntaxNode, hit: &Hit) -> u32 {
    if hit.access == Access::Write {
        if let Some(assign) = assignment_of(root, hit) {
            return end(&assign);
        }
    }
    u32::from(hit.range.start())
}

fn assignment_of(root: &SyntaxNode, hit: &Hit) -> Option<SyntaxNode> {
    let token = root.covering_element(hit.range).into_token()?;
    let mut node = token.parent()?;
    while matches!(node.kind(), VARIABLE_EXPR | ARRAY_ITEM | ARRAY_EXPR | LIST_EXPR) {
        let parent = node.parent()?;
        if parent.kind() == ASSIGN_EXPR {
            return Some(parent);
        }
        node = parent;
    }
    None
}

/// The part of the code that sees a fresh value after this write, when the hit sets the variable
/// without looking at what it was.
fn fresh_write_scope(root: &SyntaxNode, hit: &Hit) -> Option<TextRange> {
    let token = root.covering_element(hit.range).into_token()?;
    let parent = token.parent()?;
    if parent.kind() == CATCH_CLAUSE {
        return Some(parent.text_range());
    }
    if let Some(assign) = assignment_of(root, hit) {
        let plain = first_token(&assign, ASSIGN).is_some()
            && assign
                .children()
                .next()
                .is_some_and(|target| target.kind() != INDEX_EXPR);
        if !plain || parent.kind() != VARIABLE_EXPR {
            return None;
        }
        let statement = assign.parent().filter(|node| node.kind() == EXPR_STATEMENT)?;
        let list = statement.parent()?;
        return Some(TextRange::new(statement.text_range().end(), list.text_range().end()));
    }
    let owner = parent.parent()?;
    if owner.kind() == FOREACH_STATEMENT && parent.kind() == VARIABLE_EXPR {
        return Some(owner.text_range());
    }
    None
}

/// How the extracted code leaves its function.
enum Exit {
    /// It runs to its end and goes on with what follows.
    Falls,
    /// Every way through it returns, with a value or without.
    Returns { with_value: bool },
}

fn exit_of(rcx: &Rcx<'_>, piece: &Piece, function: &SyntaxNode) -> Result<Exit, String> {
    let Piece::Statements(statements) = piece else {
        return Ok(Exit::Falls);
    };
    let returns = own_nodes(piece, function, &[RETURN_STATEMENT]);
    if returns.is_empty() {
        return Ok(Exit::Falls);
    }
    if statements_complete(&rcx.cx, statements) || !statements_leave(&rcx.cx, statements) {
        return Err("The selection returns on some paths only".to_string());
    }
    let with_value = returns.iter().any(|statement| statement.children().next().is_some());
    if with_value && returns.iter().any(|statement| statement.children().next().is_none()) {
        return Err("The selection returns with a value and without".to_string());
    }
    Ok(Exit::Returns { with_value })
}

fn extract(rcx: &Rcx<'_>) -> Result<Change, String> {
    let cx = &rcx.cx;
    let (piece, function) = piece_of(rcx).ok_or("Select whole statements or one expression")?;
    if !matches!(function.kind(), METHOD_DECLARATION | FUNCTION_DECLARATION) {
        return Err("Only the code of a method or a function can be extracted".to_string());
    }
    if function
        .ancestors()
        .skip(1)
        .any(|node| ast::is_function_like(node.kind()))
    {
        return Err("The function is declared inside another one".to_string());
    }
    let class = function
        .ancestors()
        .skip(1)
        .find(|node| ast::is_class_like(node.kind()));
    if let Some(class) = &class {
        if class.kind() == INTERFACE_DECLARATION {
            return Err("An interface has no bodies".to_string());
        }
    }
    check_cuttable(&piece)?;
    let yields = !own_nodes(&piece, &function, &[YIELD_EXPR, YIELD_FROM_EXPR]).is_empty();
    let exit = exit_of(rcx, &piece, &function)?;
    let flows = flows(rcx, &piece, &function)?;
    let terminates = matches!(&piece, Piece::Statements(statements) if matches!(exit, Exit::Falls) && statements_leave(&rcx.cx, statements));
    let outputs: Vec<&Flow> = match exit {
        Exit::Falls => flows.iter().filter(|flow| flow.output).collect(),
        Exit::Returns { .. } => Vec::new(),
    };
    if yields && (!outputs.is_empty() || matches!(exit, Exit::Returns { .. })) {
        return Err("The selection yields and also hands values back".to_string());
    }
    if let Piece::Expression(expr) = &piece {
        let writes = expr.descendants().any(|node| {
            matches!(node.kind(), ASSIGN_EXPR)
                || matches!(node.kind(), PREFIX_EXPR | POSTFIX_EXPR) && (has_token(&node, INC) || has_token(&node, DEC))
        });
        if flows.iter().any(|flow| flow.output || flow.by_reference) || writes && flows.iter().any(|flow| flow.input) {
            return Err("The expression changes variables that are used afterwards".to_string());
        }
        if yields {
            return Err("An expression that yields cannot be moved into a method".to_string());
        }
    }
    if terminates && cx.index.level < php_syntax::PhpVersion::V8_1 && child_of(&function, RETURN_TYPE).is_some() {
        return Err("Code that always throws cannot be moved out before PHP 8.1".to_string());
    }
    let uses_this = nodes_of(&piece).iter().any(|node| {
        node.descendants_with_tokens().any(|element| match element {
            SyntaxElement::Token(token) => token.kind() == VARIABLE && token.text() == "$this",
            SyntaxElement::Node(_) => false,
        })
    });
    let uses_class = uses_this || nodes_of(&piece).iter().any(super::exprs::uses_class_context);
    let is_method = function.kind() == METHOD_DECLARATION;
    let is_static = is_method
        && ast::first_token(
            &child_of(&function, MODIFIER_LIST).unwrap_or_else(|| function.clone()),
            STATIC_KW,
        )
        .is_some();
    if !is_method && uses_class {
        return Err("The selection uses a class and the function has none".to_string());
    }
    let inputs: Vec<&Flow> = flows.iter().filter(|flow| flow.input).collect();
    let analyzer = cx.file.analyzer(&function);
    let first_node = nodes_of(&piece).into_iter().next().ok_or("There is nothing selected")?;
    let env = analyzer.env_around(&first_node);
    let typed_style = child_of(&function, RETURN_TYPE).is_some()
        || child_of(&function, PARAMETER_LIST).is_some_and(|list| {
            list.children().any(|parameter| {
                parameter.kind() == PARAMETER && parameter.children().any(|child| ast::is_type_node(child.kind()))
            })
        });
    let level = cx.index.level;
    let mut writer = ClassWriter::new(cx.index, &analyzer.resolver, cx.text, &cx.root, start(&function));
    let mut written = |name: &str| writer.written(name);
    let param_type = |name: &str, written: &mut dyn FnMut(&str) -> String| -> Option<String> {
        if !typed_style {
            return None;
        }
        if inputs.iter().any(|flow| flow.name == name && flow.untouched_parameter) {
            let own = parameters(&function)
                .into_iter()
                .find(|parameter| parameter_name(parameter) == name)?;
            if super::signature::is_variadic(&own) {
                return Some("array".to_string());
            }
            let declared = own.children().find(|child| ast::is_type_node(child.kind()))?;
            return Some(text_slice(cx.text, &declared).to_string());
        }
        let ty = env.get(name)?;
        if !is_plain_type(ty) {
            return None;
        }
        type_text(ty, level, written, false)
    };
    let mut params: Vec<String> = Vec::new();
    let mut doc_params: Vec<(String, String)> = Vec::new();
    for flow in &inputs {
        let mut text = String::new();
        if let Some(ty) = param_type(&flow.name, &mut written) {
            text.push_str(&ty);
            text.push(' ');
        }
        if flow.by_reference {
            text.push('&');
        }
        text.push('$');
        text.push_str(&flow.name);
        params.push(text);
        if let Some(ty) = env
            .get(&flow.name)
            .filter(|ty| !ty.is_unknown() && !matches!(ty, Type::Never | Type::Void))
        {
            if let Some(text) = doc_type(ty, &mut written) {
                doc_params.push((flow.name.clone(), text));
            }
        }
    }
    let after_env = {
        let (_, to) = span_of(&piece);
        analyzer.env_at(to)
    };
    // The type of what comes back.
    let return_type: Option<String> = if !typed_style {
        None
    } else {
        match (&piece, &exit) {
            (_, Exit::Returns { with_value: false }) => type_text(&Type::Void, level, &mut written, true),
            (_, Exit::Returns { with_value: true }) => declared_return(&function),
            (Piece::Expression(expr), _) => {
                let ty = analyzer.type_of(expr, &env);
                if ty == Type::Void {
                    type_text(&Type::Void, level, &mut written, true)
                } else {
                    sure_type(cx, &analyzer, &env, expr).and_then(|ty| type_text(&ty, level, &mut written, true))
                }
            }
            (Piece::Statements(_), Exit::Falls) => match outputs.as_slice() {
                [] if yields => declared_return(&function),
                [] if terminates => type_text(&Type::Never, level, &mut written, true),
                [] => type_text(&Type::Void, level, &mut written, true),
                [only] => after_env
                    .get(&only.name)
                    .filter(|ty| is_plain_type(ty))
                    .and_then(|ty| type_text(ty, level, &mut written, true)),
                _ => Some("array".to_string()),
            },
        }
    };
    let doc_return: Option<String> = match (&piece, &exit) {
        (Piece::Expression(expr), _) => {
            let ty = analyzer.type_of(expr, &env);
            if ty.is_unknown() || matches!(ty, Type::Void | Type::Never) {
                None
            } else {
                doc_type(&ty, &mut written)
            }
        }
        (Piece::Statements(_), Exit::Falls) => match outputs.as_slice() {
            [only] => after_env
                .get(&only.name)
                .filter(|ty| !ty.is_unknown() && !matches!(ty, Type::Never | Type::Void))
                .and_then(|ty| doc_type(ty, &mut written)),
            _ => None,
        },
        _ => None,
    };
    let void_expression = matches!(&piece, Piece::Expression(expr) if analyzer.type_of(expr, &env) == Type::Void);
    let imports = writer.into_imports();

    let class_node = class.clone();
    let taken_methods = method_names(rcx, class_node.as_ref());
    let base_name = match (&piece, outputs.as_slice()) {
        (Piece::Expression(expr), _) if !void_expression => format!("get{}", upper_first(&variable_name(cx, expr))),
        (Piece::Statements(_), [only]) => format!("get{}", upper_first(&camel(&only.name))),
        _ => "extracted".to_string(),
    };
    let name = unique_name(&base_name, &taken_methods);
    let args: Vec<String> = inputs.iter().map(|flow| format!("${}", flow.name)).collect();
    let target = if is_method {
        if is_static { "self::" } else { "$this->" }
    } else {
        ""
    };
    let call = format!("{target}{name}({})", args.join(", "));

    // The code that goes in the body, and what replaces it.
    let unit = indent_unit(cx.text, &function, rcx);
    let member_indent = indent_of(cx.text, start(&function) as usize);
    let body_indent = format!("{member_indent}{unit}");
    let (body, replacement) = match &piece {
        Piece::Expression(expr) => {
            let text = super::exprs::text_slice(cx.text, expr);
            let body = if void_expression {
                format!("{body_indent}{text};")
            } else {
                format!("{body_indent}return {text};")
            };
            (body, call.clone())
        }
        Piece::Statements(_) => {
            let (from, to) = span_of(&piece);
            let (from, to) = cut_of(cx.text, rcx.range, from, to);
            let original_indent = indent_of(cx.text, from as usize);
            let mut body = reindent(cx, from as usize, to as usize, &original_indent, &body_indent);
            let call_site = match &exit {
                Exit::Returns { with_value: true } => format!("return {call};"),
                Exit::Returns { with_value: false } => format!("{call};\n{original_indent}return;"),
                Exit::Falls if yields => format!("yield from {call};"),
                Exit::Falls => match outputs.as_slice() {
                    [] => format!("{call};"),
                    [only] => format!("${} = {call};", only.name),
                    many => {
                        let list: Vec<String> = many.iter().map(|flow| format!("${}", flow.name)).collect();
                        format!("[{}] = {call};", list.join(", "))
                    }
                },
            };
            match outputs.as_slice() {
                [only] => body.push_str(&format!("\n{body_indent}return ${};", only.name)),
                [] => {}
                many => {
                    let list: Vec<String> = many.iter().map(|flow| format!("${}", flow.name)).collect();
                    body.push_str(&format!("\n{body_indent}return [{}];", list.join(", ")));
                }
            }
            (body, call_site)
        }
    };

    let header = {
        let mut text = String::new();
        if is_method {
            text.push_str(Visibility::Private.keyword());
            text.push(' ');
            if is_static {
                text.push_str("static ");
            }
        }
        text.push_str(&format!("function {}({})", focus(&name), params.join(", ")));
        if let Some(ret) = &return_type {
            text.push_str(&format!(": {ret}"));
        }
        text
    };
    let brace_on_next_line = child_of(&function, BLOCK).is_none_or(|block| {
        let header_end = start(&block) as usize;
        let before = &cx.text[start(&function) as usize..header_end];
        before.trim_end_matches([' ', '\t']).ends_with('\n')
    });
    let doc = documentation(
        &function,
        class.as_ref(),
        &member_indent,
        &doc_params,
        &return_type,
        &doc_return,
    );
    let method = if brace_on_next_line {
        format!("{doc}{member_indent}{header}\n{member_indent}{{\n{body}\n{member_indent}}}")
    } else {
        format!("{doc}{member_indent}{header} {{\n{body}\n{member_indent}}}")
    };

    let mut draft = Draft::new(rcx.renv);
    let (from, to) = span_of(&piece);
    let (from, to) = match &piece {
        Piece::Statements(_) => cut_of(cx.text, rcx.range, from, to),
        Piece::Expression(_) => (from, to),
    };
    let replaced = TextRange::new(from.into(), to.into());
    draft.here(replace(replaced, replacement));
    draft.here(insert(end(&function), format!("\n\n{method}")));
    draft.here_all(imports);
    draft.finish()
}

/// The code that is cut out of a run of statements: the statements and the comments the selection
/// takes along around them.
fn cut_of(text: &str, selection: TextRange, first: u32, last: u32) -> (u32, u32) {
    let trimmed = trim_range(text, selection);
    (
        u32::from(trimmed.start()).min(first),
        u32::from(trimmed.end()).max(last),
    )
}

fn upper_first(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn declared_return(function: &SyntaxNode) -> Option<String> {
    let ty = child_of(function, RETURN_TYPE)?.children().next()?;
    Some(ast::text_of(&ty))
}

/// The members of a written union once, and `mixed` alone when it is one of them. A type that
/// would need parentheses is not worth writing.
pub(super) fn flatten_union(text: &str) -> Option<String> {
    if text.contains('(') || text.contains(')') {
        return None;
    }
    let nullable = text.strip_prefix('?');
    let body = nullable.unwrap_or(text);
    if body.contains('&') && body.contains('|') {
        return None;
    }
    let mut members: Vec<&str> = Vec::new();
    for member in body.split('|') {
        if !members.contains(&member) {
            members.push(member);
        }
    }
    if members.contains(&"mixed") {
        return Some("mixed".to_string());
    }
    let joined = members.join("|");
    Some(match nullable {
        Some(_) if members.len() == 1 => format!("?{joined}"),
        Some(_) => format!("{joined}|null"),
        None => joined,
    })
}

/// A type as a PHP version can write it, or nothing when it cannot be written there.
fn type_text(
    ty: &Type,
    level: php_syntax::PhpVersion,
    class: &mut dyn FnMut(&str) -> String,
    is_return: bool,
) -> Option<String> {
    if ty.is_unknown() {
        return None;
    }
    let probe = flatten_union(&native_type_text(ty, &mut |name: &str| name.to_string())?)?;
    let at_least = |version: php_syntax::PhpVersion| level >= version;
    if probe.contains('|') && !probe.ends_with("|null") && !at_least(php_syntax::PhpVersion::V8_0) {
        return None;
    }
    if probe.contains('|') && probe.ends_with("|null") && !at_least(php_syntax::PhpVersion::V8_0) {
        return None;
    }
    if (probe == "mixed" || probe.contains("static") && is_return) && !at_least(php_syntax::PhpVersion::V8_0) {
        return None;
    }
    if probe == "never" && !(is_return && at_least(php_syntax::PhpVersion::V8_1)) {
        return None;
    }
    if probe == "mixed" && !is_return {
        return None;
    }
    if matches!(probe.as_str(), "null" | "false" | "true") && !at_least(php_syntax::PhpVersion::V8_2) {
        return None;
    }
    if probe.contains('&') && !at_least(php_syntax::PhpVersion::V8_1) {
        return None;
    }
    // The classes are written only now, so a type that was turned down leaves no import behind.
    let text = native_type_text(ty, class)?;
    flatten_union(&text)
}

/// A type as a doc block writes it: whatever the type layer knows, as long as it reads plainly.
fn doc_type(ty: &Type, class: &mut dyn FnMut(&str) -> String) -> Option<String> {
    let probe = ty.display_with(&mut |name: &str| name.to_string());
    if probe.contains('(') || probe.contains("never") || probe.contains("mixed|") || probe.contains("|mixed") {
        return None;
    }
    let text = ty.display_with(class);
    Some(if text.contains(['<', '{']) {
        text
    } else {
        flatten_union(&text)?
    })
}

/// A type that code can be trusted to have: a scalar or an array, or either or `null`. What the type
/// layer reads from a doc block or from the way objects flow is not enough for a declaration, which
/// PHP checks.
fn is_plain_type(ty: &Type) -> bool {
    match ty {
        Type::Int
        | Type::Float
        | Type::String
        | Type::Bool
        | Type::True
        | Type::False
        | Type::IntLiteral(_)
        | Type::StringLiteral(_)
        | Type::Array(..)
        | Type::List(_)
        | Type::Shape(_) => true,
        Type::Union(members) => {
            members.len() == 2
                && members.contains(&Type::Null)
                && members
                    .iter()
                    .all(|member| *member == Type::Null || is_plain_type(member))
        }
        _ => false,
    }
}

/// The names of the methods or functions that the new name must not take.
fn method_names(rcx: &Rcx<'_>, class: Option<&SyntaxNode>) -> Vec<String> {
    let cx = &rcx.cx;
    let mut out: Vec<String> = Vec::new();
    match class {
        Some(class) => {
            if let Some(body) = child_of(class, CLASS_BODY) {
                for member in body.children().filter(|member| member.kind() == METHOD_DECLARATION) {
                    if let Some(name) = child_of(&member, NAME) {
                        out.push(text_of(&name));
                    }
                }
            }
            let analyzer = cx.file.analyzer(class);
            if let Some(context) = &analyzer.class {
                for found in cx.index.methods(&Type::class(context.name.clone())) {
                    out.push(found.member.name.clone());
                }
            }
        }
        None => {
            for node in cx.nodes.iter().filter(|node| node.kind() == FUNCTION_DECLARATION) {
                if let Some(name) = child_of(node, NAME) {
                    out.push(text_of(&name));
                }
            }
        }
    }
    out
}

fn unique_name(base: &str, taken: &[String]) -> String {
    let set: std::collections::HashSet<String> = taken.iter().map(|name| name.to_ascii_lowercase()).collect();
    if !set.contains(&base.to_ascii_lowercase()) {
        return base.to_string();
    }
    (2..)
        .map(|suffix| format!("{base}{suffix}"))
        .find(|candidate| !set.contains(&candidate.to_ascii_lowercase()))
        .unwrap_or_else(|| base.to_string())
}

fn indent_unit(text: &str, function: &SyntaxNode, rcx: &Rcx<'_>) -> String {
    let own = indent_of(text, start(function) as usize);
    if let Some(block) = child_of(function, BLOCK) {
        if let Some(first) = block.children().next() {
            let inner = indent_of(text, start(&first) as usize);
            if inner.len() > own.len() && inner.starts_with(&own) {
                return inner[own.len()..].to_string();
            }
        }
    }
    rcx.renv.format.indent.unit()
}

/// The code with its lines moved from one indentation to another. The lines inside strings and
/// heredocs are the strings' own.
fn reindent(cx: &crate::inspections::Cx, from_offset: usize, to_offset: usize, from: &str, to: &str) -> String {
    let code = &cx.text[from_offset..to_offset];
    let protected: Vec<(usize, usize)> = cx
        .root
        .descendants()
        .filter(|node| matches!(node.kind(), INTERPOLATED_STRING | HEREDOC | SHELL_EXEC_EXPR | LITERAL))
        .map(|node| (start(&node) as usize, end(&node) as usize))
        .filter(|&(s, e)| e > from_offset && s < to_offset && cx.text[s..e].contains('\n'))
        .collect();
    let mut out = String::new();
    let mut at = from_offset;
    for (index, line) in code.split_inclusive('\n').enumerate() {
        let line_start = at;
        at += line.len();
        let inside_string = protected.iter().any(|&(s, e)| line_start > s && line_start < e);
        if index == 0 {
            out.push_str(to);
            out.push_str(line);
        } else if inside_string || line.trim().is_empty() {
            out.push_str(line);
        } else if let Some(rest) = line.strip_prefix(from) {
            out.push_str(to);
            out.push_str(rest);
        } else {
            out.push_str(line.trim_start_matches([' ', '\t']));
        }
    }
    out
}

fn documentation(
    function: &SyntaxNode,
    class: Option<&SyntaxNode>,
    indent: &str,
    params: &[(String, String)],
    return_type: &Option<String>,
    returned: &Option<String>,
) -> String {
    if !project_writes_docs(function, class) {
        return String::new();
    }
    let mut lines: Vec<String> = Vec::new();
    for (name, ty) in params {
        lines.push(format!("@param {ty} ${name}"));
    }
    if let Some(ty) = returned {
        if return_type.as_deref() != Some(ty.as_str()) || ty.contains('<') {
            lines.push(format!("@return {ty}"));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!("{indent}/**\n");
    for line in lines {
        out.push_str(&format!("{indent} * {line}\n"));
    }
    out.push_str(&format!("{indent} */\n"));
    out
}

/// Whether the doc blocks of this project's methods are worth carrying on: the function has one,
/// or more than half of the methods of its class do.
fn project_writes_docs(function: &SyntaxNode, class: Option<&SyntaxNode>) -> bool {
    let has_doc = |node: &SyntaxNode| {
        ast::tokens(node)
            .take_while(|token| token.kind() != FUNCTION_KW)
            .any(|token| token.kind() == DOC_COMMENT)
    };
    if has_doc(function) {
        return true;
    }
    let Some(body) = class.and_then(|class| child_of(class, CLASS_BODY)) else {
        return false;
    };
    let methods: Vec<SyntaxNode> = body
        .children()
        .filter(|member| member.kind() == METHOD_DECLARATION)
        .collect();
    let documented = methods.iter().filter(|method| has_doc(method)).count();
    !methods.is_empty() && documented * 2 > methods.len()
}
