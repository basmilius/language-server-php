//! Naming the arguments of a call, and giving the names up again.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::{around, caret, rewrite};
use crate::actions::edits::{delete, insert};
use crate::ast::{has_token, start};
use crate::refactor::draft::Draft;
use crate::refactor::signature::{Argument, arguments_of};
use crate::refactor::{Rcx, Refactor};

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let Some(argument) = around(&token).find(|node| node.kind() == ARGUMENT) else {
        return;
    };
    let Some(call) = argument
        .parent()
        .and_then(|list| list.parent())
        .filter(|call| matches!(call.kind(), CALL_EXPR | NEW_EXPR))
    else {
        return;
    };
    if rcx.cx.index.level < php_syntax::PhpVersion::V8_0 {
        return;
    }
    let named = has_token(&argument, COLON);
    if named {
        if names_in_order(rcx, &call).is_ok() {
            let call = call.clone();
            out.push(rewrite(
                format!("rewrite-unname@{}", start(&call)),
                "Remove the argument names",
                move || {
                    let mut draft = Draft::new(rcx.renv);
                    for edit in remove_names(&call)? {
                        draft.here(edit);
                    }
                    draft.finish()
                },
            ));
        }
        return;
    }
    if add_names(rcx, &call, &argument).is_ok() {
        out.push(rewrite(
            format!("rewrite-name@{}", start(&argument)),
            "Add argument names",
            move || {
                let mut draft = Draft::new(rcx.renv);
                draft.here_all(add_names(rcx, &call, &argument)?);
                draft.finish()
            },
        ));
    }
}

/// The parameter names of the one function a call reaches, in order, and which one is variadic.
fn parameters_of(rcx: &Rcx<'_>, call: &SyntaxNode) -> Result<Vec<(String, bool)>, String> {
    let analyzer = rcx.cx.file.analyzer(call);
    let env = analyzer.env_around(call);
    let callees = analyzer.callees(call, &env);
    let mut found: Option<Vec<(String, bool)>> = None;
    for callee in callees {
        let names: Vec<(String, bool)> = callee
            .callable
            .params_at(rcx.cx.index.level)
            .map(|param| (param.name.clone(), param.variadic))
            .collect();
        match &found {
            Some(known) if *known != names => {
                return Err("The call may reach functions that name their parameters differently".to_string());
            }
            _ => found = Some(names),
        }
    }
    found.ok_or_else(|| "The function is not known".to_string())
}

fn add_names(rcx: &Rcx<'_>, call: &SyntaxNode, from: &SyntaxNode) -> Result<Vec<crate::completion::TextEdit>, String> {
    let parameters = parameters_of(rcx, call)?;
    let given: Vec<Argument> = arguments_of(call);
    if given.iter().any(|arg| arg.spread) {
        return Err("The call spreads its arguments".to_string());
    }
    let position = given
        .iter()
        .position(|arg| arg.node == *from)
        .ok_or("The argument is gone")?;
    if given.iter().take(position).any(|arg| arg.name.is_some()) {
        return Err("A name comes before it".to_string());
    }
    let mut edits = Vec::new();
    for (index, arg) in given.iter().enumerate().skip(position) {
        if arg.name.is_some() {
            continue;
        }
        let Some((name, variadic)) = parameters.get(index) else {
            return Err("There are more arguments than parameters".to_string());
        };
        if *variadic {
            return Err("A variadic parameter takes no name".to_string());
        }
        let first = arg.value.as_ref().map_or(start(&arg.node), start);
        edits.push(insert(first, format!("{name}: ")));
    }
    if edits.is_empty() {
        return Err("Nothing to name".to_string());
    }
    Ok(edits)
}

/// Whether every name stands where its parameter does, so that without it the argument still lands there.
fn names_in_order(rcx: &Rcx<'_>, call: &SyntaxNode) -> Result<(), String> {
    let parameters = parameters_of(rcx, call)?;
    let given = arguments_of(call);
    let mut named_seen = false;
    for (index, arg) in given.iter().enumerate() {
        match &arg.name {
            Some(name) => {
                named_seen = true;
                if parameters.get(index).map(|(parameter, _)| parameter) != Some(name) {
                    return Err("An argument is not where its parameter is".to_string());
                }
            }
            None if named_seen => return Err("A positional argument follows a named one".to_string()),
            None => {}
        }
    }
    Ok(())
}

fn remove_names(call: &SyntaxNode) -> Result<Vec<crate::completion::TextEdit>, String> {
    let mut edits = Vec::new();
    for arg in arguments_of(call) {
        let Some(value) = &arg.value else {
            continue;
        };
        if arg.name.is_some() {
            edits.push(delete(TextRange::new(start(&arg.node).into(), start(value).into())));
        }
    }
    Ok(edits)
}
