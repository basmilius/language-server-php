//! Extract variable: an expression becomes a variable assigned right before its statement, and the
//! expression, or every equal one in reach, reads the variable instead.

use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::draft::{Draft, focus};
use super::exprs::{
    expression_of_selection, expressions_at, has_side_effects, hoist_site, insertion_before, replaceable, scope_of,
    token_text,
};
use super::names::{taken_variables, unique, variable_name};
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::actions::edits::replace;
use crate::ast::{end, start};
use crate::completion::TextEdit;
use crate::refs::is_write_target;

/// The most inner expressions offered next to the outermost one.
const MAX_INNER: usize = 2;

/// What the cursor may mean: an expression whose value is worth a name.
fn is_sensible(expr: &SyntaxNode) -> bool {
    let kind_ok = match expr.kind() {
        VARIABLE_EXPR | VARIABLE_VARIABLE | ASSIGN_EXPR | LIST_EXPR | THROW_EXPR | EXIT_EXPR => false,
        NAME => !matches!(
            crate::ast::text_of(expr).to_ascii_lowercase().as_str(),
            "true" | "false" | "null"
        ),
        _ => true,
    };
    if !kind_ok || !is_replaceable(expr) {
        return false;
    }
    let Some(parent) = expr.parent() else {
        return false;
    };
    let whole_statement = parent.kind() == EXPR_STATEMENT;
    let assigned_whole = parent.kind() == ASSIGN_EXPR
        && parent.children().nth(1).as_ref() == Some(expr)
        && parent.parent().is_some_and(|grand| grand.kind() == EXPR_STATEMENT);
    !whole_statement && !assigned_whole
}

/// Whether a variable can stand where the expression is.
pub(crate) fn is_replaceable(expr: &SyntaxNode) -> bool {
    replaceable(expr).is_ok()
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let cx = &rcx.cx;
    let explicit = !rcx.range.is_empty();
    let candidates: Vec<SyntaxNode> = if explicit {
        expression_of_selection(&cx.root, cx.text, rcx.range)
            .into_iter()
            .collect()
    } else {
        expressions_at(&cx.root, start_of(rcx))
            .into_iter()
            .filter(|expr| is_sensible(expr) && hoist_site(expr).is_ok())
            .collect()
    };
    let Some(outermost) = candidates.last().cloned() else {
        return;
    };
    if explicit && hoist_site(&outermost).is_err() {
        out.push(single(rcx, &outermost, "Extract variable".to_string(), false));
        return;
    }
    out.push(single(rcx, &outermost, "Extract variable".to_string(), false));
    let count = occurrences(&outermost).map_or(0, |found| found.len());
    if count > 1 {
        out.push(single(
            rcx,
            &outermost,
            format!("Extract variable, replacing all {count} occurrences"),
            true,
        ));
    }
    for inner in candidates.iter().rev().skip(1).take(MAX_INNER) {
        out.push(single(
            rcx,
            inner,
            format!("Extract variable from '{}'", preview(rcx.cx.text, inner)),
            false,
        ));
    }
}

fn start_of(rcx: &Rcx<'_>) -> u32 {
    u32::from(rcx.range.start())
}

fn preview(text: &str, expr: &SyntaxNode) -> String {
    let source: String = super::exprs::text_slice(text, expr)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if source.chars().count() > 32 {
        let cut: String = source.chars().take(31).collect();
        format!("{cut}...")
    } else {
        source
    }
}

fn single<'a>(rcx: &'a Rcx<'a>, expr: &SyntaxNode, title: String, all: bool) -> Refactor<'a> {
    let (from, to) = (start(expr), end(expr));
    let id = format!("extract-variable{}@{from}-{to}", if all { "-all" } else { "" });
    let expr = expr.clone();
    Refactor::new(id, title, RefactorKind::Extract, true, move || extract(rcx, &expr, all))
}

/// The expressions equal to this one that a variable assigned before the first of them can stand
/// for, in the order of the file.
fn occurrences(expr: &SyntaxNode) -> Result<Vec<SyntaxNode>, String> {
    if has_side_effects(expr) {
        return Err("An expression that does something cannot be run once for all its places".to_string());
    }
    let scope = scope_of(expr);
    let wanted = token_text(expr);
    let mut found: Vec<SyntaxNode> = scope
        .descendants()
        .filter(|node| node.kind() == expr.kind() && super::exprs::is_expression(node) && token_text(node) == wanted)
        .filter(|node| is_replaceable(node) && same_function(node, expr))
        .collect();
    found.sort_by_key(start);
    let mut kept: Vec<SyntaxNode> = Vec::new();
    for node in found {
        if kept.last().is_some_and(|last| end(&node) <= end(last)) {
            continue;
        }
        kept.push(node);
    }
    let Some(first) = kept.first() else {
        return Ok(kept);
    };
    let site = hoist_site(first)?;
    let from = start(&site.statement);
    let container = site.container.clone();
    let reads = super::exprs::variables_in(expr);
    let stateful = expr.descendants().any(|node| {
        matches!(
            node.kind(),
            PROPERTY_FETCH_EXPR | INDEX_EXPR | STATIC_PROPERTY_EXPR | SCOPED_ACCESS_EXPR | CALL_EXPR
        )
    });
    kept.retain(|node| {
        container.text_range().contains_range(node.text_range())
            && start(node) >= from
            && !invalidated_before(&scope, node, from, &reads, stateful)
    });
    Ok(kept)
}

fn same_function(node: &SyntaxNode, expr: &SyntaxNode) -> bool {
    crate::ast::enclosing_function(node) == crate::ast::enclosing_function(expr)
}

/// Whether the value of the expression may differ between where the variable is assigned and a place.
fn invalidated_before(scope: &SyntaxNode, place: &SyntaxNode, from: u32, reads: &[String], stateful: bool) -> bool {
    let mut to = start(place);
    let loops = place.ancestors().filter(|node| {
        matches!(
            node.kind(),
            WHILE_STATEMENT | DO_WHILE_STATEMENT | FOR_STATEMENT | FOREACH_STATEMENT
        ) && start(node) >= from
    });
    let mut origin = from;
    if let Some(outer) = loops.last() {
        origin = start(&outer);
        to = to.max(end(&outer));
    }
    scope.descendants().any(|node| {
        let at = start(&node);
        if at < origin || at >= to || at < from && at < origin {
            return false;
        }
        match node.kind() {
            VARIABLE_EXPR => {
                is_write_target(&node)
                    && reads.iter().any(|name| {
                        super::exprs::variables_in(&node)
                            .first()
                            .is_some_and(|written| written == name)
                    })
            }
            CALL_EXPR | NEW_EXPR | INCLUDE_EXPR | EVAL_EXPR => stateful || reads_by_reference(&node, reads),
            ASSIGN_EXPR | PREFIX_EXPR | POSTFIX_EXPR => {
                stateful && is_write_target(node.children().next().as_ref().unwrap_or(&node))
            }
            _ => false,
        }
    })
}

/// A call that may write a variable the expression reads, by taking it by reference.
fn reads_by_reference(call: &SyntaxNode, reads: &[String]) -> bool {
    super::exprs::variables_in(call).iter().any(|name| reads.contains(name))
}

fn extract(rcx: &Rcx<'_>, expr: &SyntaxNode, all: bool) -> Result<Change, String> {
    let cx = &rcx.cx;
    replaceable(expr)?;
    if expr.parent().is_some_and(|parent| parent.kind() == ARGUMENT)
        && super::scope::by_reference_variables(cx, &scope_of(expr)).contains(&start(expr))
    {
        return Err("The expression is taken by reference".to_string());
    }
    {
        let analyzer = cx.file.analyzer(expr);
        let env = analyzer.env_around(expr);
        if analyzer.type_of(expr, &env) == php_index::Type::Never {
            return Err("The expression never returns, so nothing could use its value".to_string());
        }
    }
    let places = if all { occurrences(expr)? } else { vec![expr.clone()] };
    let Some(first) = places.first() else {
        return Err("There is nothing to replace".to_string());
    };
    let site = hoist_site(first)?;
    let taken = taken_variables(&scope_of(expr));
    let name = unique(&variable_name(cx, first), &taken);
    let value = value_text(cx.text, first);
    let declaration = format!("${} = {value};", focus(&name));
    let insertion = insertion_before(cx.text, &site.statement, &declaration);
    let mut draft = Draft::new(rcx.renv);
    draft.here(TextEdit {
        start: insertion.offset,
        end: insertion.offset,
        new_text: insertion.text,
    });
    for place in &places {
        draft.here(replace(place.text_range(), format!("${name}")));
    }
    draft.finish()
}

/// The text of an expression for the right side of an assignment: a parenthesized one without them.
fn value_text(text: &str, expr: &SyntaxNode) -> String {
    if expr.kind() == PAREN_EXPR {
        if let Some(inner) = expr.children().next() {
            return super::exprs::text_slice(text, &inner).to_string();
        }
    }
    super::exprs::text_slice(text, expr).to_string()
}
