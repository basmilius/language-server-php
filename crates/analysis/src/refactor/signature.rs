//! Changing the signature of a function or method without a dialog: add a parameter with a
//! default, remove one that nothing uses, move one past its neighbor. Every declaration that is
//! one thing with it (overrides, implementations) and every call that is found is changed
//! together, or nothing is.

use std::path::PathBuf;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode, TextRange, parse};

use super::draft::{Draft, focus};
use super::exprs::{has_side_effects, token_near};
use super::names::unique;
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::actions::edits::{delete, indent_of, insert, replace};
use crate::ast::{self, child_of, end, first_token, has_token, start, text_of, tokens};
use crate::completion::TextEdit;
use crate::decl::declarations;
use crate::inspections::Cx;
use crate::references::{Current, hits_of_symbols};
use crate::refs::{HitKind, Query, Symbol, variable_hits};

/// A file as the refactor reads it.
#[derive(Clone)]
pub(crate) struct Loaded {
    pub path: PathBuf,
    pub text: String,
    pub root: SyntaxNode,
}

/// A function or method that is one thing with the one asked about.
pub(crate) struct Declared {
    pub file: usize,
    pub function: SyntaxNode,
}

/// A call or `new` that reaches the function or one of its overrides.
pub(crate) struct CallSite {
    pub file: usize,
    pub call: SyntaxNode,
}

pub(crate) struct Family {
    pub files: Vec<Loaded>,
    pub declarations: Vec<Declared>,
    pub calls: Vec<CallSite>,
    /// References that are not calls, which a change of the arguments would leave behind.
    pub loose: usize,
}

pub(crate) fn is_callable_declaration(node: &SyntaxNode) -> bool {
    matches!(node.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION)
}

pub(crate) fn parameters(function: &SyntaxNode) -> Vec<SyntaxNode> {
    child_of(function, PARAMETER_LIST)
        .map(|list| list.children().filter(|child| child.kind() == PARAMETER).collect())
        .unwrap_or_default()
}

pub(crate) fn parameter_name(parameter: &SyntaxNode) -> String {
    first_token(parameter, VARIABLE).map_or_else(String::new, |token| token.text().trim_start_matches('$').to_string())
}

pub(crate) fn is_variadic(parameter: &SyntaxNode) -> bool {
    has_token(parameter, ELLIPSIS)
}

/// The text a parameter gives as its default.
pub(crate) fn default_of(text: &str, parameter: &SyntaxNode) -> Option<String> {
    let value = parameter.children().last().filter(|node| {
        first_token(parameter, ASSIGN).is_some_and(|assign| assign.text_range().end() <= node.text_range().start())
    })?;
    Some(text[start(&value) as usize..end(&value) as usize].to_string())
}

/// The arguments of a call, with the name one is given by.
pub(crate) struct Argument {
    pub node: SyntaxNode,
    pub name: Option<String>,
    pub spread: bool,
    pub value: Option<SyntaxNode>,
}

pub(crate) fn arguments_of(call: &SyntaxNode) -> Vec<Argument> {
    let Some(list) = child_of(call, ARGUMENT_LIST) else {
        return Vec::new();
    };
    list.children()
        .filter(|child| child.kind() == ARGUMENT)
        .map(|node| {
            let name = has_token(&node, COLON).then(|| {
                tokens(&node)
                    .find(|token| !token.kind().is_trivia() && token.kind() != COLON)
                    .map(|token| token.text().to_string())
                    .unwrap_or_default()
            });
            Argument {
                name,
                spread: has_token(&node, ELLIPSIS),
                value: node.children().last(),
                node,
            }
        })
        .collect()
}

/// Whether the call is the first-class callable syntax `f(...)`, which has no arguments to change.
fn is_first_class(call: &SyntaxNode) -> bool {
    child_of(call, ARGUMENT_LIST)
        .is_some_and(|list| list.children().next().is_none() && tokens(&list).any(|token| token.kind() == ELLIPSIS))
}

/// The call a name stands in, when it is what is called.
fn call_of_name(name: &SyntaxNode) -> Option<SyntaxNode> {
    let parent = name.parent()?;
    match parent.kind() {
        CALL_EXPR if parent.children().next().as_ref() == Some(name) => Some(parent),
        NEW_EXPR => Some(parent),
        PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR => {
            let call = parent.parent()?;
            (call.kind() == CALL_EXPR && call.children().next().as_ref() == Some(&parent)).then_some(call)
        }
        _ => None,
    }
}

/// The symbol a declaration is, as the index knows it.
fn symbol_of(cx: &Cx, root: &SyntaxNode, function: &SyntaxNode) -> Result<Symbol, String> {
    let name = child_of(function, NAME)
        .map(|name| text_of(&name))
        .ok_or("The declaration has no name")?;
    let analyzer = crate::context::FileContext::new(cx.index, root).analyzer(function);
    if function.kind() == FUNCTION_DECLARATION {
        return Ok(Symbol::Function(analyzer.resolver.qualify(&name)));
    }
    let class = function
        .ancestors()
        .skip(1)
        .find(|node| ast::is_class_like(node.kind()))
        .ok_or("The method has no class")?;
    if class.kind() == ANONYMOUS_CLASS {
        return Err("A method of an anonymous class has no other users".to_string());
    }
    let class_name = child_of(&class, NAME)
        .map(|name| analyzer.resolver.qualify(&text_of(&name)))
        .ok_or("The class has no name")?;
    Ok(Symbol::Method {
        class: class_name,
        name,
    })
}

/// Finds the declarations and the calls that belong together with a function.
pub(crate) fn family(rcx: &Rcx<'_>, function: &SyntaxNode) -> Result<Family, String> {
    let here = Loaded {
        path: rcx.renv.path.to_path_buf(),
        text: rcx.cx.text.to_string(),
        root: rcx.cx.root.clone(),
    };
    family_in(rcx, &here, function)
}

/// Like [`family`], for a function of any file.
pub(crate) fn family_in(rcx: &Rcx<'_>, home: &Loaded, function: &SyntaxNode) -> Result<Family, String> {
    let cx = &rcx.cx;
    let symbol = symbol_of(cx, &home.root, function)?;
    let query = Query::new(cx.index, symbol.clone());
    if let Some(outside) = declarations(cx.index, &query)
        .iter()
        .find(|declaration| declaration.origin != php_index::Origin::Project)
    {
        return Err(format!(
            "{} is declared in a package or the standard library",
            outside.path.display()
        ));
    }
    let current = Current {
        path: rcx.renv.path,
        text: cx.text,
        root: &cx.root,
    };
    let found = hits_of_symbols(cx.index, rcx.renv.sources, &current, &[symbol]);
    let mut files: Vec<Loaded> = Vec::new();
    let mut declared: Vec<Declared> = Vec::new();
    let mut calls: Vec<CallSite> = Vec::new();
    let mut loose = 0;
    for file in found {
        let (text, root) = if file.path == rcx.renv.path {
            (cx.text.to_string(), cx.root.clone())
        } else {
            let Some(text) = rcx.renv.sources.text(&file.path) else {
                continue;
            };
            let root = parse(&text).syntax();
            (text, root)
        };
        let index = files.len();
        files.push(Loaded {
            path: file.path.clone(),
            text,
            root: root.clone(),
        });
        for hit in &file.hits {
            let Some(token) = root.covering_element(hit.range).into_token() else {
                continue;
            };
            let Some(name) = token.parent().filter(|parent| parent.kind() == NAME) else {
                continue;
            };
            match hit.kind {
                HitKind::Declaration => {
                    if let Some(owner) = name.parent().filter(is_callable_declaration) {
                        if !declared
                            .iter()
                            .any(|known| known.file == index && known.function == owner)
                        {
                            declared.push(Declared {
                                file: index,
                                function: owner,
                            });
                        }
                    }
                }
                HitKind::Reference => match call_of_name(&name) {
                    Some(call) if is_first_class(&call) => {}
                    Some(call) => {
                        if !calls.iter().any(|known| known.file == index && known.call == call) {
                            calls.push(CallSite { file: index, call });
                        }
                    }
                    None => loose += 1,
                },
                HitKind::Import | HitKind::Doc => {}
            }
        }
    }
    if !declared
        .iter()
        .any(|known| files[known.file].path == home.path && known.function.text_range() == function.text_range())
    {
        let index = files.iter().position(|file| file.path == home.path).unwrap_or_else(|| {
            files.push(home.clone());
            files.len() - 1
        });
        declared.push(Declared {
            file: index,
            function: function.clone(),
        });
    }
    Ok(Family {
        files,
        declarations: declared,
        calls,
        loose,
    })
}

// Editing a list of items between parentheses ---------------------------------------------------

/// The edit that adds an item at the end of a parameter or argument list.
pub(crate) fn append_item(text: &str, list: &SyntaxNode, item: &str, item_kind: SyntaxKind) -> TextEdit {
    let items: Vec<SyntaxNode> = list.children().filter(|child| child.kind() == item_kind).collect();
    let Some(last) = items.last() else {
        let open = tokens(list).find(|token| token.kind() == LPAREN);
        let at = open.map_or(start(list) + 1, |token| u32::from(token.text_range().end()));
        return insert(at, item);
    };
    let multiline = text[start(list) as usize..end(list) as usize].contains('\n')
        && text[crate::actions::edits::line_start(text, start(last) as usize)..start(last) as usize]
            .trim()
            .is_empty();
    let separator = if multiline {
        format!(",\n{}", indent_of(text, start(last) as usize))
    } else {
        ", ".to_string()
    };
    let trailing =
        tokens(list).find(|token| token.kind() == COMMA && token.text_range().start() >= last.text_range().end());
    match trailing {
        Some(comma) => {
            let rest = separator.trim_start_matches(',').to_string();
            insert(u32::from(comma.text_range().end()), format!("{rest}{item},"))
        }
        None => insert(end(last), format!("{separator}{item}")),
    }
}

/// The edit that takes the item at a position out of a parameter or argument list.
pub(crate) fn remove_item(list: &SyntaxNode, position: usize, item_kind: SyntaxKind) -> Option<TextEdit> {
    let items: Vec<SyntaxNode> = list.children().filter(|child| child.kind() == item_kind).collect();
    let item = items.get(position)?;
    if items.len() == 1 {
        let trailing =
            tokens(list).find(|token| token.kind() == COMMA && token.text_range().start() >= item.text_range().end());
        let to = trailing.map_or(end(item), |comma| u32::from(comma.text_range().end()));
        return Some(delete(TextRange::new(start(item).into(), to.into())));
    }
    Some(match items.get(position + 1) {
        Some(next) => delete(TextRange::new(start(item).into(), start(next).into())),
        None => {
            let previous = &items[position - 1];
            let trailing = tokens(list)
                .find(|token| token.kind() == COMMA && token.text_range().start() >= item.text_range().end());
            let to = trailing.map_or(end(item), |comma| u32::from(comma.text_range().end()));
            delete(TextRange::new(
                previous.text_range().end(),
                to.min(end(item)).max(end(item)).into(),
            ))
        }
    })
}

// Intentions ------------------------------------------------------------------------------------

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let cx = &rcx.cx;
    if !rcx.range.is_empty() {
        return;
    }
    let offset = u32::from(rcx.range.start());
    let Some(token) = token_near(&cx.root, offset) else {
        return;
    };
    let Some(owner) = token.parent() else {
        return;
    };
    let parameter = owner.ancestors().find(|node| node.kind() == PARAMETER);
    let function = match &parameter {
        Some(parameter) => parameter.parent().and_then(|list| list.parent()),
        None => owner.ancestors().find(is_callable_declaration),
    };
    let Some(function) = function.filter(is_callable_declaration) else {
        return;
    };
    let in_header = child_of(&function, BLOCK).is_none_or(|body| offset <= start(&body));
    if !in_header {
        return;
    }
    let at = start(&function);
    if parameter.is_none() {
        let on_name = child_of(&function, NAME).is_some_and(|name| name.text_range().contains_inclusive(offset.into()));
        let in_list =
            child_of(&function, PARAMETER_LIST).is_some_and(|list| list.text_range().contains_inclusive(offset.into()));
        if !on_name && !in_list {
            return;
        }
        let function = function.clone();
        out.push(Refactor::new(
            format!("signature-add@{at}"),
            "Add parameter",
            RefactorKind::Rewrite,
            true,
            move || add_parameter(rcx, &function),
        ));
        return;
    }
    let Some(parameter) = parameter else {
        return;
    };
    let all = parameters(&function);
    let Some(position) = all.iter().position(|candidate| *candidate == parameter) else {
        return;
    };
    let name = parameter_name(&parameter);
    if name.is_empty() || has_modifier_list(&parameter) {
        return;
    }
    let function_for_add = function.clone();
    out.push(Refactor::new(
        format!("signature-add@{at}"),
        "Add parameter",
        RefactorKind::Rewrite,
        true,
        move || add_parameter(rcx, &function_for_add),
    ));
    if is_unused(rcx, &function, &name) {
        let function = function.clone();
        out.push(Refactor::new(
            format!("signature-remove@{at}:{position}"),
            format!("Remove unused parameter '${name}'"),
            RefactorKind::Rewrite,
            true,
            move || remove_parameter(rcx, &function, position),
        ));
    }
    if position > 0 && !is_variadic(&parameter) {
        let function = function.clone();
        out.push(Refactor::new(
            format!("signature-move@{at}:{position}:left"),
            format!("Move parameter '${name}' left"),
            RefactorKind::Rewrite,
            true,
            move || move_parameter(rcx, &function, position, position - 1),
        ));
    }
    if position + 1 < all.len() && !is_variadic(&parameter) && !is_variadic(&all[position + 1]) {
        let function = function.clone();
        out.push(Refactor::new(
            format!("signature-move@{at}:{position}:right"),
            format!("Move parameter '${name}' right"),
            RefactorKind::Rewrite,
            true,
            move || move_parameter(rcx, &function, position, position + 1),
        ));
    }
}

fn has_modifier_list(parameter: &SyntaxNode) -> bool {
    child_of(parameter, MODIFIER_LIST).is_some()
}

/// Whether nothing in the function reads the parameter.
fn is_unused(rcx: &Rcx<'_>, function: &SyntaxNode, name: &str) -> bool {
    let hits = variable_hits(&rcx.cx.file, (start(function), end(function)), name);
    hits.iter()
        .all(|hit| hit.kind == HitKind::Declaration || hit.kind == HitKind::Doc)
}

fn param_edits_for(
    draft: &mut Draft<'_>,
    family: &Family,
    edit: impl Fn(&Loaded, &SyntaxNode) -> Result<Vec<TextEdit>, String>,
) -> Result<(), String> {
    for declared in &family.declarations {
        let file = &family.files[declared.file];
        let edits = edit(file, &declared.function)?;
        draft.edits(&file.path, edits);
    }
    Ok(())
}

fn add_parameter(rcx: &Rcx<'_>, function: &SyntaxNode) -> Result<Change, String> {
    let family = family(rcx, function)?;
    let mut taken: Vec<String> = Vec::new();
    for declared in &family.declarations {
        if parameters(&declared.function).iter().any(is_variadic) {
            return Err("A parameter cannot be added before a variadic one".to_string());
        }
        taken.extend(super::names::taken_variables(&declared.function));
    }
    let set: std::collections::HashSet<String> = taken.into_iter().collect();
    let name = unique("parameter", &set);
    let mut draft = Draft::new(rcx.renv);
    let current = rcx.renv.path;
    param_edits_for(&mut draft, &family, |file, declared| {
        let list = child_of(declared, PARAMETER_LIST).ok_or("The declaration has no parameter list")?;
        let shown = if file.path == current && *declared == *function {
            focus(&name)
        } else {
            name.clone()
        };
        let mut edits = vec![append_item(&file.text, &list, &format!("${shown} = null"), PARAMETER)];
        edits.extend(document_parameter(&file.text, declared, &name, None));
        Ok(edits)
    })?;
    draft.finish()
}

/// The `@param` line for a new parameter, after the ones the doc block already has.
pub(crate) fn document_parameter(text: &str, function: &SyntaxNode, name: &str, ty: Option<&str>) -> Option<TextEdit> {
    let doc = tokens(function)
        .take_while(|token| token.kind() != FUNCTION_KW)
        .find(|token| token.kind() == DOC_COMMENT)?;
    let body = doc.text();
    let last_param = body
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains("@param"))
        .last();
    let (line_index, _) = last_param?;
    let base = u32::from(doc.text_range().start()) as usize;
    let mut offset = base;
    for (index, line) in body.split_inclusive('\n').enumerate() {
        if index == line_index {
            let prefix: String = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let at = (offset + line.trim_end_matches(['\n', '\r']).len()) as u32;
            let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
            return Some(insert(
                at,
                format!("{eol}{prefix}* @param {} ${name}", ty.unwrap_or("mixed")),
            ));
        }
        offset += line.len();
    }
    None
}

fn remove_parameter(rcx: &Rcx<'_>, function: &SyntaxNode, position: usize) -> Result<Change, String> {
    let family = family(rcx, function)?;
    if family.loose > 0 {
        return Err("The function is also used where it is not called, which this would break".to_string());
    }
    let own = parameters(function);
    let name = own.get(position).map(parameter_name).ok_or("The parameter is gone")?;
    let mut draft = Draft::new(rcx.renv);
    for declared in &family.declarations {
        let file = &family.files[declared.file];
        let all = parameters(&declared.function);
        let Some(target) = all.get(position) else {
            return Err("An override has fewer parameters".to_string());
        };
        if has_modifier_list(target) {
            return Err("An override promotes the parameter to a property".to_string());
        }
        let used = variable_hits(
            &crate::context::FileContext::new(rcx.cx.index, &file.root),
            (start(&declared.function), end(&declared.function)),
            &parameter_name(target),
        )
        .iter()
        .any(|hit| hit.kind != HitKind::Declaration && hit.kind != HitKind::Doc);
        if used {
            return Err(format!(
                "An override still uses the parameter in {}",
                file.path.display()
            ));
        }
        let list = child_of(&declared.function, PARAMETER_LIST).ok_or("The declaration has no parameter list")?;
        draft.edits(&file.path, remove_item(&list, position, PARAMETER));
        draft.edits(&file.path, doc_line_of(&declared.function, &parameter_name(target)));
    }
    for site in &family.calls {
        let file = &family.files[site.file];
        let given = arguments_of(&site.call);
        if given.iter().take(position + 1).any(|arg| arg.spread) {
            return Err(format!("A call in {} spreads its arguments", file.path.display()));
        }
        let by_name = given.iter().position(|arg| arg.name.as_deref() == Some(name.as_str()));
        let at = by_name.or_else(|| {
            let positional = given.iter().take_while(|arg| arg.name.is_none()).count();
            (position < positional).then_some(position)
        });
        let Some(at) = at else {
            continue;
        };
        if given[at].value.as_ref().is_some_and(has_side_effects) {
            return Err(format!(
                "A call in {} passes something that does something",
                file.path.display()
            ));
        }
        let list = child_of(&site.call, ARGUMENT_LIST).ok_or("The call has no arguments")?;
        draft.edits(&file.path, remove_item(&list, at, ARGUMENT));
    }
    draft.finish()
}

/// The deletion of the `@param` line of a parameter.
fn doc_line_of(function: &SyntaxNode, name: &str) -> Option<TextEdit> {
    let doc = tokens(function)
        .take_while(|token| token.kind() != FUNCTION_KW)
        .find(|token| token.kind() == DOC_COMMENT)?;
    let base = u32::from(doc.text_range().start()) as usize;
    let mut offset = base;
    let wanted = format!("${name}");
    for line in doc.text().split_inclusive('\n') {
        let words: Vec<&str> = line.split(|c: char| c.is_whitespace()).collect();
        if line.contains("@param") && words.iter().any(|word| word.trim_end_matches(',') == wanted) {
            return Some(delete(TextRange::new(
                (offset as u32).into(),
                ((offset + line.len()) as u32).into(),
            )));
        }
        offset += line.len();
    }
    None
}

/// A default that reads the same wherever the call is.
fn portable_default(text: &str) -> bool {
    let trimmed = text.trim();
    matches!(trimmed, "null" | "true" | "false" | "[]" | "NULL" | "TRUE" | "FALSE")
        || trimmed.parse::<f64>().is_ok()
        || (trimmed.len() >= 2
            && (trimmed.starts_with('\'') && trimmed.ends_with('\'')
                || trimmed.starts_with('"') && trimmed.ends_with('"'))
            && !trimmed.contains('$'))
}

fn move_parameter(rcx: &Rcx<'_>, function: &SyntaxNode, from: usize, to: usize) -> Result<Change, String> {
    let family = family(rcx, function)?;
    if family.loose > 0 {
        return Err("The function is also used where it is not called, which this would break".to_string());
    }
    let (low, high) = (from.min(to), from.max(to));
    let own = parameters(function);
    let own_text = &family.files[family
        .declarations
        .iter()
        .find(|declared| declared.function == *function)
        .map_or(0, |declared| declared.file)]
    .text;
    let (low_name, high_name) = (parameter_name(&own[low]), parameter_name(&own[high]));
    let optional = |parameter: &SyntaxNode| default_of(own_text, parameter).is_some() || is_variadic(parameter);
    let mut order: Vec<&SyntaxNode> = own.iter().collect();
    order.swap(low, high);
    let mut seen_optional = false;
    for parameter in &order {
        if optional(parameter) {
            seen_optional = true;
        } else if seen_optional {
            return Err("A parameter without a default cannot come after one that has".to_string());
        }
    }
    let high_default = default_of(own_text, &own[high]);
    let mut draft = Draft::new(rcx.renv);
    for declared in &family.declarations {
        let file = &family.files[declared.file];
        let all = parameters(&declared.function);
        if all.len() <= high {
            return Err("An override has fewer parameters".to_string());
        }
        let (first, second) = (&all[low], &all[high]);
        let (first_text, second_text) = (
            file.text[start(first) as usize..end(first) as usize].to_string(),
            file.text[start(second) as usize..end(second) as usize].to_string(),
        );
        draft.edit(&file.path, replace(first.text_range(), second_text));
        draft.edit(&file.path, replace(second.text_range(), first_text));
    }
    let level = rcx.cx.index.level;
    for site in &family.calls {
        let file = &family.files[site.file];
        let given = arguments_of(&site.call);
        if given.iter().any(|arg| arg.spread) {
            return Err(format!("A call in {} spreads its arguments", file.path.display()));
        }
        let positional: Vec<&Argument> = given.iter().take_while(|arg| arg.name.is_none()).collect();
        let named = |name: &str| given.iter().any(|arg| arg.name.as_deref() == Some(name));
        match (positional.get(low), positional.get(high)) {
            (Some(first), Some(second)) => {
                let (first_text, second_text) = (
                    file.text[start(&first.node) as usize..end(&first.node) as usize].to_string(),
                    file.text[start(&second.node) as usize..end(&second.node) as usize].to_string(),
                );
                draft.edit(&file.path, replace(first.node.text_range(), second_text));
                draft.edit(&file.path, replace(second.node.text_range(), first_text));
            }
            (Some(first), None) => {
                let first_text = &file.text[start(&first.node) as usize..end(&first.node) as usize];
                let after = if named(&high_name) || positional.len() > high {
                    None
                } else {
                    high_default.as_deref().filter(|text| portable_default(text))
                };
                let text = match after {
                    Some(default) if !named(&high_name) => format!("{default}, {first_text}"),
                    _ if level >= php_syntax::PhpVersion::V8_0 => format!("{low_name}: {first_text}"),
                    _ => {
                        return Err(format!(
                            "A call in {} leaves out an argument the new order needs",
                            file.path.display()
                        ));
                    }
                };
                draft.edit(&file.path, replace(first.node.text_range(), text));
            }
            _ => {}
        }
    }
    draft.finish()
}
