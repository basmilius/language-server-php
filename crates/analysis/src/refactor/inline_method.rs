//! Inline method: a call is replaced by the body of the function it calls, with the arguments
//! where the parameters were. A body that is one `return` goes anywhere the call stands; a few
//! statements go where the call is a statement of its own. The method goes with its last call.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode, TextRange, parse};

use super::draft::Draft;
use super::exprs::{has_side_effects, hoist_site, is_constant_like, token_near, uses_class_context};
use super::inline_variable::needs_parentheses;
use super::names::unique;
use super::signature::{
    Argument, Loaded, arguments_of, default_of, family_in, is_callable_declaration, is_variadic, parameter_name,
    parameters,
};
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::actions::edits::{indent_of, replace};
use crate::ast::{self, child_of, end, first_token, has_token, range_of, start, text_of};
use crate::decl::declarations;
use crate::references::symbols_at;
use crate::refs::{Query, Symbol};

/// The most statements of a body that are worth copying to the calls.
const MAX_STATEMENTS: usize = 10;

/// What a body is made of.
enum Form {
    /// One `return` and its expression.
    Expression(SyntaxNode),
    /// Statements, and the expression of a last `return` when there is one.
    Statements {
        statements: Vec<SyntaxNode>,
        result: Option<SyntaxNode>,
    },
}

struct Callee {
    /// Where the names of the body are read: set when the body is copied to another file.
    file: Loaded,
    function: SyntaxNode,
    form: Form,
    params: Vec<SyntaxNode>,
    class: Option<SyntaxNode>,
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    if !rcx.range.is_empty() {
        return;
    }
    let cx = &rcx.cx;
    let offset = u32::from(rcx.range.start());
    let Some(token) = token_near(&cx.root, offset) else {
        return;
    };
    let Some(name) = token.parent().filter(|parent| parent.kind() == NAME) else {
        return;
    };
    let on_declaration = name.parent().filter(is_callable_declaration);
    let (callee, call) = match on_declaration {
        Some(function) => {
            let file = loaded_here(rcx);
            match prepare(&file, &function) {
                Ok(callee) => (callee, None),
                Err(_) => return,
            }
        }
        None => {
            let Some(call) = call_of_name(&name) else {
                return;
            };
            match callee_of_call(rcx, offset) {
                Some(callee) => (callee, Some(call)),
                None => return,
            }
        }
    };
    let at = start(&callee.function);
    let callee = Rc::new(callee);
    let for_all = callee.clone();
    out.push(Refactor::new(
        format!("inline-method@{at}"),
        "Inline method",
        RefactorKind::Inline,
        true,
        move || inline_all(rcx, &for_all),
    ));
    if let Some(call) = call {
        out.push(Refactor::new(
            format!("inline-call@{}", start(&call)),
            "Inline this call",
            RefactorKind::Inline,
            true,
            move || inline_one(rcx, &callee, &call),
        ));
    }
}

fn loaded_here(rcx: &Rcx<'_>) -> Loaded {
    Loaded {
        path: rcx.renv.path.to_path_buf(),
        text: rcx.cx.text.to_string(),
        root: rcx.cx.root.clone(),
    }
}

fn call_of_name(name: &SyntaxNode) -> Option<SyntaxNode> {
    let parent = name.parent()?;
    match parent.kind() {
        CALL_EXPR if parent.children().next().as_ref() == Some(name) => Some(parent),
        PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR => {
            let call = parent.parent()?;
            (call.kind() == CALL_EXPR && call.children().next().as_ref() == Some(&parent)).then_some(call)
        }
        _ => None,
    }
}

/// The function a call at an offset of the current file calls, when there is exactly one.
fn callee_of_call(rcx: &Rcx<'_>, offset: u32) -> Option<Callee> {
    let cx = &rcx.cx;
    let (_, symbols) = symbols_at(cx.index, &cx.root, offset)?;
    let symbol = symbols
        .into_iter()
        .find(|symbol| matches!(symbol, Symbol::Method { .. } | Symbol::Function(_)))?;
    let found = declarations(cx.index, &Query::new(cx.index, symbol));
    let [declaration] = found.as_slice() else {
        return None;
    };
    if declaration.origin != php_index::Origin::Project {
        return None;
    }
    let file = if declaration.path == rcx.renv.path {
        loaded_here(rcx)
    } else {
        let text = rcx.renv.sources.text(&declaration.path)?;
        let root = parse(&text).syntax();
        Loaded {
            path: declaration.path.clone(),
            text,
            root,
        }
    };
    let name = file
        .root
        .covering_element(range_of(declaration.name_span.start, declaration.name_span.end))
        .into_token()?
        .parent()?;
    let function = name.parent().filter(is_callable_declaration)?;
    prepare(&file, &function).ok()
}

/// Checks that a function can be copied to its calls and reads what its body is.
fn prepare(file: &Loaded, function: &SyntaxNode) -> Result<Callee, String> {
    let body = child_of(function, BLOCK).ok_or("The function has no body")?;
    if has_modifier(function, ABSTRACT_KW) {
        return Err("An abstract method has no body".to_string());
    }
    let params = parameters(function);
    for parameter in &params {
        if is_variadic(parameter) || has_token(parameter, AMP) {
            return Err("A variadic or by-reference parameter cannot be inlined".to_string());
        }
        if child_of(parameter, MODIFIER_LIST).is_some() {
            return Err("A promoted parameter cannot be inlined".to_string());
        }
    }
    if first_token(function, AMP).is_some_and(|amp| amp.text_range().start() < body.text_range().start()) {
        return Err("A function that returns by reference cannot be inlined".to_string());
    }
    let own = |node: &SyntaxNode| ast::enclosing_function(node).as_ref() == Some(function);
    let name = child_of(function, NAME).map(|name| text_of(&name)).unwrap_or_default();
    for node in body.descendants() {
        match node.kind() {
            YIELD_EXPR | YIELD_FROM_EXPR if own(&node) => return Err("A generator cannot be inlined".to_string()),
            STATIC_VARIABLE_STATEMENT
            | GLOBAL_STATEMENT
            | GOTO_STATEMENT
            | LABEL_STATEMENT
            | FUNCTION_DECLARATION
            | CLASS_DECLARATION
            | INTERFACE_DECLARATION
            | TRAIT_DECLARATION
            | ENUM_DECLARATION
            | VARIABLE_VARIABLE
            | EVAL_EXPR
            | INCLUDE_EXPR
            | TRY_STATEMENT => {
                return Err("The body does something that cannot be copied".to_string());
            }
            CALL_EXPR => {
                if let Some(callee) = node.children().next() {
                    let called = match callee.kind() {
                        NAME => text_of(&callee),
                        _ => callee
                            .children()
                            .filter(|child| child.kind() == NAME)
                            .last()
                            .map(|name| text_of(&name))
                            .unwrap_or_default(),
                    };
                    let lower = called.trim_start_matches('\\').to_ascii_lowercase();
                    if matches!(
                        lower.as_str(),
                        "compact" | "extract" | "get_defined_vars" | "func_get_args" | "func_get_arg" | "func_num_args"
                    ) {
                        return Err("The body reads its variables by name".to_string());
                    }
                    if called.eq_ignore_ascii_case(&name) {
                        return Err("The function calls itself".to_string());
                    }
                }
            }
            _ => {}
        }
    }
    let names: HashSet<String> = params.iter().map(parameter_name).collect();
    for node in body
        .descendants()
        .filter(|node| matches!(node.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
    {
        if super::exprs::variables_in(&node)
            .iter()
            .any(|name| names.contains(name))
        {
            return Err("A closure in the body uses a parameter".to_string());
        }
    }
    let statements: Vec<SyntaxNode> = body.children().collect();
    if statements.is_empty() {
        return Err("The body is empty".to_string());
    }
    if statements.len() > MAX_STATEMENTS {
        return Err("The body is too long to copy".to_string());
    }
    let returns: Vec<SyntaxNode> = body
        .descendants()
        .filter(|node| node.kind() == RETURN_STATEMENT && own(node))
        .collect();
    let form = match statements.as_slice() {
        [only] if only.kind() == RETURN_STATEMENT => {
            let expr = only.children().next().ok_or("The function returns nothing")?;
            Form::Expression(expr)
        }
        _ => {
            let last = statements.last().filter(|last| last.kind() == RETURN_STATEMENT);
            let allowed = usize::from(last.is_some());
            if returns.len() != allowed {
                return Err("The function can return before its end".to_string());
            }
            match last {
                Some(last) => Form::Statements {
                    statements: statements[..statements.len() - 1].to_vec(),
                    result: last.children().next(),
                },
                None => Form::Statements {
                    statements,
                    result: None,
                },
            }
        }
    };
    let class = function
        .ancestors()
        .skip(1)
        .find(|node| ast::is_class_like(node.kind()));
    if class
        .as_ref()
        .is_some_and(|class| class.kind() == INTERFACE_DECLARATION)
    {
        return Err("An interface has no bodies".to_string());
    }
    Ok(Callee {
        file: file.clone(),
        function: function.clone(),
        form,
        params,
        class,
    })
}

fn has_modifier(function: &SyntaxNode, kind: php_syntax::SyntaxKind) -> bool {
    child_of(function, MODIFIER_LIST).is_some_and(|list| has_token(&list, kind))
}

// Doing it ------------------------------------------------------------------------------------

fn inline_all(rcx: &Rcx<'_>, callee: &Callee) -> Result<Change, String> {
    let family = family_in(rcx, &callee.file, &callee.function)?;
    if family.declarations.len() > 1 {
        return Err("The method is overridden or implemented elsewhere".to_string());
    }
    if family.loose > 0 {
        return Err("The method is also used where it is not called".to_string());
    }
    if family.calls.is_empty() {
        return Err("Nothing calls the method".to_string());
    }
    let mut draft = Draft::new(rcx.renv);
    for (position, site) in family.calls.iter().enumerate() {
        let caller = &family.files[site.file];
        if family.calls.iter().enumerate().any(|(other, found)| {
            other != position
                && found.file == site.file
                && found.call.text_range().contains_range(site.call.text_range())
        }) {
            return Err("A call is inside the arguments of another".to_string());
        }
        let edits = inline_site(rcx, callee, caller, &site.call)?;
        draft.edits(&caller.path, edits);
    }
    let declared_in = &family.files[family.declarations[0].file];
    draft.edit(
        &declared_in.path,
        super::exprs::remove_member(&declared_in.text, callee.function.text_range()),
    );
    draft.finish()
}

fn inline_one(rcx: &Rcx<'_>, callee: &Callee, call: &SyntaxNode) -> Result<Change, String> {
    let caller = loaded_here(rcx);
    let mut draft = Draft::new(rcx.renv);
    draft.here_all(inline_site(rcx, callee, &caller, call)?);
    draft.finish()
}

/// How a call stands in its statement.
enum Place {
    /// `call;`
    Statement(SyntaxNode),
    /// `$target = call;`
    Assigned { statement: SyntaxNode, target: String },
    /// `return call;`
    Returned(SyntaxNode),
    /// Inside a bigger expression.
    Nested,
}

fn place_of(text: &str, call: &SyntaxNode) -> Place {
    let Some(parent) = call.parent() else {
        return Place::Nested;
    };
    match parent.kind() {
        EXPR_STATEMENT if super::exprs::is_statement_position(&parent) => Place::Statement(parent),
        RETURN_STATEMENT if super::exprs::is_statement_position(&parent) => Place::Returned(parent),
        ASSIGN_EXPR
            if parent.children().nth(1).as_ref() == Some(call)
                && first_token(&parent, ASSIGN).is_some()
                && parent
                    .children()
                    .next()
                    .is_some_and(|target| target.kind() == VARIABLE_EXPR) =>
        {
            match parent.parent().filter(|statement| {
                statement.kind() == EXPR_STATEMENT && super::exprs::is_statement_position(statement)
            }) {
                Some(statement) => {
                    let target = parent.children().next().map_or(String::new(), |target| {
                        text[start(&target) as usize..end(&target) as usize].to_string()
                    });
                    Place::Assigned { statement, target }
                }
                None => Place::Nested,
            }
        }
        _ => Place::Nested,
    }
}

/// What stands in for a parameter in the body.
struct Binding {
    /// What is written where the parameter was.
    text: String,
    /// The expression it came from, for the parentheses it may need.
    value: Option<SyntaxNode>,
    /// Anything that can be written twice and read the same.
    trivial: bool,
    effects: bool,
}

fn bind(callee: &Callee, caller: &Loaded, call: &SyntaxNode) -> Result<Vec<(String, Binding)>, String> {
    let given: Vec<Argument> = arguments_of(call);
    if given.iter().any(|arg| arg.spread) {
        return Err("A call spreads its arguments".to_string());
    }
    let mut out = Vec::new();
    let positional: Vec<&Argument> = given.iter().take_while(|arg| arg.name.is_none()).collect();
    for (position, parameter) in callee.params.iter().enumerate() {
        let name = parameter_name(parameter);
        let argument = given
            .iter()
            .find(|arg| arg.name.as_deref() == Some(name.as_str()))
            .or_else(|| positional.get(position).copied());
        match argument.and_then(|arg| arg.value.clone()) {
            Some(value) => {
                let text = caller.text[start(&value) as usize..end(&value) as usize].to_string();
                out.push((
                    name,
                    Binding {
                        trivial: is_trivial(&value),
                        effects: has_side_effects(&value),
                        text,
                        value: Some(value),
                    },
                ));
            }
            None => {
                let default = default_of(&callee.file.text, parameter)
                    .ok_or("A call leaves out an argument that has no default")?;
                if callee.file.path != caller.path
                    && !default
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || " '\"-.[]_".contains(c))
                {
                    return Err("A default that names something cannot move to another file".to_string());
                }
                out.push((
                    name,
                    Binding {
                        trivial: true,
                        effects: false,
                        text: default,
                        value: None,
                    },
                ));
            }
        }
    }
    if given.len() > callee.params.len()
        && given
            .iter()
            .skip(callee.params.len())
            .any(|arg| arg.value.as_ref().is_some_and(has_side_effects))
    {
        return Err("A call passes more than the function takes, and does something with it".to_string());
    }
    Ok(out)
}

/// A value that reads the same wherever it is written: a variable, a literal, a constant.
fn is_trivial(value: &SyntaxNode) -> bool {
    matches!(value.kind(), VARIABLE_EXPR | LITERAL | NAME)
        || is_constant_like(value)
            && value
                .descendants()
                .all(|node| node.kind() != NAME || is_plain_constant(&node))
}

fn is_plain_constant(_: &SyntaxNode) -> bool {
    true
}

/// The edits that put the body of the callee where one call stands.
fn inline_site(
    rcx: &Rcx<'_>,
    callee: &Callee,
    caller: &Loaded,
    call: &SyntaxNode,
) -> Result<Vec<crate::completion::TextEdit>, String> {
    check_receiver(callee, caller, call)?;
    let bindings = bind(callee, caller, call)?;
    let caller_scope = ast::enclosing_function(call).unwrap_or_else(|| caller.root.clone());
    let taken = super::names::taken_variables(&caller_scope);
    let place = place_of(&caller.text, call);
    let names = (callee.file.path != caller.path).then(|| Names {
        index: rcx.cx.index,
        resolver: php_index::extract::resolver_at(&callee.file.root, start(&callee.function)),
    });
    let fresh = |name: &str, used: &mut HashSet<String>| -> String {
        let mut all = taken.clone();
        all.extend(used.iter().cloned());
        let chosen = unique(name, &all);
        used.insert(chosen.clone());
        chosen
    };
    let mut used: HashSet<String> = HashSet::new();
    match &callee.form {
        Form::Expression(expr) => {
            let written = written_in_expression(callee, names.as_ref(), expr, &bindings)?;
            let text = if needs_parentheses(expr, call) {
                format!("({written})")
            } else {
                written
            };
            Ok(vec![replace(call.text_range(), text)])
        }
        Form::Statements { statements, result } => {
            let at_statement = match &place {
                Place::Statement(statement) | Place::Returned(statement) => statement.clone(),
                Place::Assigned { statement, .. } => statement.clone(),
                Place::Nested => {
                    return Err("The body has statements, so the call has to be a statement of its own".to_string());
                }
            };
            let indent = indent_of(&caller.text, start(&at_statement) as usize);
            let mut renames: HashMap<String, String> = HashMap::new();
            let mut lines: Vec<String> = Vec::new();
            let param_names: HashSet<String> = bindings.iter().map(|(name, _)| name.clone()).collect();
            let writes: HashSet<String> = written_params(callee, &param_names);
            let mut substitutions: HashMap<String, (String, Option<SyntaxNode>)> = HashMap::new();
            for (name, binding) in &bindings {
                let reads = count_reads(callee, name);
                let direct = binding.trivial && !writes.contains(name) && !binding.effects;
                if direct || reads == 0 && !binding.effects {
                    substitutions.insert(name.clone(), (binding.text.clone(), binding.value.clone()));
                } else {
                    let local = fresh(name, &mut used);
                    lines.push(format!("${local} = {};", binding.text));
                    if local != *name {
                        renames.insert(name.clone(), local);
                    }
                }
            }
            for local in locals_of(callee, &param_names) {
                if taken.contains(&local) {
                    let renamed = fresh(&local, &mut used);
                    renames.insert(local, renamed);
                }
            }
            let body = written_statements(callee, names.as_ref(), statements, &substitutions, &renames, &indent)?;
            let mut text = lines.join(&format!("\n{indent}"));
            if !text.is_empty() {
                text.push_str(&format!("\n{indent}"));
            }
            text.push_str(body.trim_start());
            if let Some(result) = result {
                let value = written_in_statements(names.as_ref(), result, &substitutions, &renames)?;
                let line = match &place {
                    Place::Statement(_) => {
                        if has_side_effects(result) {
                            format!("{value};")
                        } else {
                            String::new()
                        }
                    }
                    Place::Assigned { target, .. } => format!("{target} = {value};"),
                    Place::Returned(_) => format!("return {value};"),
                    Place::Nested => String::new(),
                };
                if !line.is_empty() {
                    if !text.is_empty() {
                        text.push_str(&format!("\n{indent}"));
                    }
                    text.push_str(&line);
                }
            } else if let Place::Assigned { target, .. } = &place {
                text.push_str(&format!("\n{indent}{target} = null;"));
            } else if let Place::Returned(_) = &place {
                text.push_str(&format!("\n{indent}return null;"));
            }
            Ok(vec![replace(at_statement.text_range(), text)])
        }
    }
}

/// Whether the call may be copied into: a body that uses its object only goes to calls on it.
fn check_receiver(callee: &Callee, caller: &Loaded, call: &SyntaxNode) -> Result<(), String> {
    let uses_class = match &callee.form {
        Form::Expression(expr) => uses_class_context(expr),
        Form::Statements { statements, result } => {
            statements.iter().any(uses_class_context) || result.as_ref().is_some_and(uses_class_context)
        }
    };
    let Some(callee_node) = call.children().next() else {
        return Err("The call has no callee".to_string());
    };
    let receiver = callee_node.children().next().filter(|_| callee_node.kind() != NAME);
    let receiver_text = receiver.as_ref().map(|node| text_of(node).to_ascii_lowercase());
    let own_receiver = matches!(receiver_text.as_deref(), Some("$this" | "self" | "static"));
    if uses_class {
        let same_class = call
            .ancestors()
            .skip(1)
            .find(|node| ast::is_class_like(node.kind()))
            .is_some_and(|class| {
                callee.class.as_ref().is_some_and(|declaring| {
                    let name_of = |node: &SyntaxNode, file: &Loaded| {
                        child_of(node, NAME).map(|name| {
                            let resolver = php_index::extract::resolver_at(&file.root, start(node));
                            resolver.qualify(&text_of(&name))
                        })
                    };
                    name_of(&class, caller).is_some() && name_of(&class, caller) == name_of(declaring, &callee.file)
                })
            });
        if !own_receiver || !same_class {
            return Err(
                "The body uses its object, so it can only go into calls on that object from its own class".to_string(),
            );
        }
    } else if let Some(receiver) = &receiver {
        if !matches!(receiver.kind(), VARIABLE_EXPR | NAME) && !own_receiver {
            return Err("The object the call is made on is worked out by the call".to_string());
        }
    }
    Ok(())
}

/// Variables a body assigns that are parameters.
fn written_params(callee: &Callee, params: &HashSet<String>) -> HashSet<String> {
    let mut out = HashSet::new();
    for token in body_tokens(callee) {
        if token.kind() != VARIABLE {
            continue;
        }
        let name = token.text().trim_start_matches('$').to_string();
        if !params.contains(&name) {
            continue;
        }
        if let Some(parent) = token.parent() {
            if crate::refs::is_write_target(&parent) {
                out.insert(name);
            }
        }
    }
    out
}

fn body_nodes(callee: &Callee) -> Vec<SyntaxNode> {
    match &callee.form {
        Form::Expression(expr) => vec![expr.clone()],
        Form::Statements { statements, result } => {
            let mut all = statements.clone();
            all.extend(result.clone());
            all
        }
    }
}

fn body_tokens(callee: &Callee) -> Vec<php_syntax::SyntaxToken> {
    body_nodes(callee)
        .iter()
        .flat_map(|node| {
            node.descendants_with_tokens()
                .filter_map(SyntaxElement::into_token)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn count_reads(callee: &Callee, name: &str) -> usize {
    let wanted = format!("${name}");
    body_tokens(callee)
        .iter()
        .filter(|token| token.kind() == VARIABLE && token.text() == wanted)
        .count()
}

/// The variables of a body that are its own: neither parameters nor `$this`.
fn locals_of(callee: &Callee, params: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for token in body_tokens(callee) {
        if token.kind() != VARIABLE {
            continue;
        }
        let name = token.text().trim_start_matches('$').to_string();
        if name == "this" || params.contains(&name) || out.contains(&name) {
            continue;
        }
        out.push(name);
    }
    out
}

/// The names of a body as they are written for the file it is copied to.
struct Names<'a> {
    index: &'a php_index::Index,
    resolver: php_index::NameResolver,
}

/// A body node's text with the parameters replaced.
fn rewrite(
    names: Option<&Names<'_>>,
    node: &SyntaxNode,
    substitutions: &HashMap<String, (String, Option<SyntaxNode>)>,
    renames: &HashMap<String, String>,
) -> Result<String, String> {
    let mut out = String::new();
    for element in node.descendants_with_tokens() {
        let SyntaxElement::Token(token) = element else {
            continue;
        };
        if token.kind() != VARIABLE {
            let written = names.and_then(|names| {
                let name = token.parent().filter(|parent| parent.kind() == NAME)?;
                super::names::fully_qualified(names.index, &names.resolver, &name)
            });
            out.push_str(written.as_deref().unwrap_or(token.text()));
            continue;
        }
        let name = token.text().trim_start_matches('$');
        let Some(parent) = token.parent() else {
            out.push_str(token.text());
            continue;
        };
        if let Some(local) = renames.get(name) {
            out.push_str(&format!("${local}"));
            continue;
        }
        let Some((text, value)) = substitutions.get(name) else {
            out.push_str(token.text());
            continue;
        };
        let in_string = parent
            .ancestors()
            .take_while(|node| !matches!(node.kind(), BLOCK | STATEMENT_LIST))
            .any(|node| matches!(node.kind(), INTERPOLATED_STRING | HEREDOC | SHELL_EXEC_EXPR));
        match value {
            _ if in_string && !text.starts_with('$') => {
                return Err("A parameter is written inside a string, which cannot hold the argument".to_string());
            }
            Some(value) if !in_string && needs_parentheses(value, &parent) => out.push_str(&format!("({text})")),
            _ if in_string && !text.starts_with('$') => {
                return Err("A parameter is written inside a string, which cannot hold the argument".to_string());
            }
            _ => out.push_str(text),
        }
    }
    Ok(out)
}

fn written_in_expression(
    callee: &Callee,
    names: Option<&Names<'_>>,
    expr: &SyntaxNode,
    bindings: &[(String, Binding)],
) -> Result<String, String> {
    let params: HashSet<String> = bindings.iter().map(|(name, _)| name.clone()).collect();
    if !written_params(callee, &params).is_empty() {
        return Err("The body assigns to a parameter".to_string());
    }
    let mut substitutions: HashMap<String, (String, Option<SyntaxNode>)> = HashMap::new();
    let mut effectful_uses: Vec<(u32, String)> = Vec::new();
    for (name, binding) in bindings {
        let reads = count_reads(callee, name);
        if binding.effects {
            if reads != 1 {
                return Err("An argument does something and is not read exactly once".to_string());
            }
            let place = expr
                .descendants()
                .find(|node| node.kind() == VARIABLE_EXPR && text_of(node) == format!("${name}"))
                .ok_or("The parameter is not read")?;
            hoist_site(&place).map_err(|_| "An argument does something and is not read first".to_string())?;
            effectful_uses.push((start(&place), name.clone()));
        } else if !binding.trivial && reads > 1 {
            // A value read twice is worked out twice, which is only the same for a value with no effects.
        }
        substitutions.insert(name.clone(), (binding.text.clone(), binding.value.clone()));
    }
    effectful_uses.sort_by_key(|(at, _)| *at);
    let order: Vec<&String> = bindings
        .iter()
        .filter(|(_, binding)| binding.effects)
        .map(|(name, _)| name)
        .collect();
    if effectful_uses.iter().map(|(_, name)| name).collect::<Vec<_>>() != order {
        return Err("The arguments are read in another order than they are given".to_string());
    }
    rewrite(names, expr, &substitutions, &HashMap::new())
}

fn written_in_statements(
    names: Option<&Names<'_>>,
    node: &SyntaxNode,
    substitutions: &HashMap<String, (String, Option<SyntaxNode>)>,
    renames: &HashMap<String, String>,
) -> Result<String, String> {
    rewrite(names, node, substitutions, renames)
}

/// The statements of a body written at the indentation of the call, with comments and the
/// layout of everything inside kept.
fn written_statements(
    callee: &Callee,
    names: Option<&Names<'_>>,
    statements: &[SyntaxNode],
    substitutions: &HashMap<String, (String, Option<SyntaxNode>)>,
    renames: &HashMap<String, String>,
    indent: &str,
) -> Result<String, String> {
    let (Some(first), Some(last)) = (statements.first(), statements.last()) else {
        return Ok(String::new());
    };
    let text = &callee.file.text;
    let original_indent = indent_of(text, start(first) as usize);
    let mut out = String::new();
    let mut previous_end = start(first);
    for statement in statements {
        out.push_str(&text[previous_end as usize..start(statement) as usize]);
        out.push_str(&rewrite(names, statement, substitutions, renames)?);
        previous_end = end(statement);
    }
    let protected: Vec<TextRange> = first
        .parent()
        .into_iter()
        .flat_map(|parent| parent.descendants())
        .filter(|node| {
            matches!(node.kind(), INTERPOLATED_STRING | HEREDOC | SHELL_EXEC_EXPR | LITERAL)
                && text[start(node) as usize..end(node) as usize].contains('\n')
        })
        .map(|node| node.text_range())
        .collect();
    let mut lines: Vec<String> = Vec::new();
    let mut at = start(first) as usize;
    let mut source_lines = Vec::new();
    for line in text[start(first) as usize..end(last) as usize].split_inclusive('\n') {
        source_lines.push((at, line.len()));
        at += line.len();
    }
    for (index, line) in out.split_inclusive('\n').enumerate() {
        let inside = source_lines.get(index).is_some_and(|&(offset, _)| {
            protected.iter().any(|range| {
                u32::try_from(offset)
                    .is_ok_and(|offset| offset > u32::from(range.start()) && offset < u32::from(range.end()))
            })
        });
        if index == 0 || inside || line.trim().is_empty() {
            lines.push(line.to_string());
        } else if let Some(rest) = line.strip_prefix(original_indent.as_str()) {
            lines.push(format!("{indent}{rest}"));
        } else {
            lines.push(format!("{indent}{}", line.trim_start_matches([' ', '\t'])));
        }
    }
    Ok(lines.concat())
}
