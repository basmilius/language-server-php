//! A `switch` as a `match`, where nothing about it can change.

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::{around, caret, eol, has_comment, rewrite, text_of_node, unit};
use crate::actions::edits::{indent_of, replace};
use crate::ast::{start, tokens};
use crate::refactor::draft::Draft;
use crate::refactor::{Rcx, Refactor};

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(token) = caret(rcx) else {
        return;
    };
    let offset = u32::from(rcx.range.start());
    let Some(switch) = around(&token).find(|node| {
        node.kind() == SWITCH_STATEMENT && {
            let open = tokens(node)
                .find(|token| token.kind() == LBRACE)
                .map_or(u32::MAX, |token| u32::from(token.text_range().start()));
            offset <= open
        }
    }) else {
        return;
    };
    if to_match(rcx, &switch).is_ok() {
        let node = switch.clone();
        out.push(rewrite(
            format!("rewrite-switch@{}", start(&switch)),
            "Convert switch to match",
            move || {
                let text = to_match(rcx, &node)?;
                let mut draft = Draft::new(rcx.renv);
                draft.here(replace(node.text_range(), text));
                draft.finish()
            },
        ));
    }
}

/// What every arm of the switch does with its value.
#[derive(PartialEq, Eq)]
enum Use {
    Return,
    Echo,
    Assign(String),
}

struct Arm {
    labels: Vec<SyntaxNode>,
    is_default: bool,
    value: SyntaxNode,
    how: Use,
}

/// The statements of a clause: the ones after the colon.
fn statements_of(clause: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut after_colon = false;
    let mut out = Vec::new();
    for element in clause.children_with_tokens() {
        match element {
            php_syntax::SyntaxElement::Token(token) if matches!(token.kind(), COLON | SEMICOLON) => after_colon = true,
            php_syntax::SyntaxElement::Node(node) if after_colon => out.push(node),
            _ => {}
        }
    }
    out
}

fn to_match(rcx: &Rcx<'_>, switch: &SyntaxNode) -> Result<String, String> {
    let text = rcx.cx.text;
    if has_comment(switch) {
        return Err("There are comments in it".to_string());
    }
    let subject = switch.children().next().ok_or("No subject")?;
    let clauses: Vec<SyntaxNode> = switch
        .children()
        .filter(|child| matches!(child.kind(), CASE_CLAUSE | DEFAULT_CLAUSE))
        .collect();
    if clauses.is_empty() {
        return Err("Nothing to match".to_string());
    }
    let mut arms: Vec<Arm> = Vec::new();
    let mut pending: Vec<SyntaxNode> = Vec::new();
    let mut pending_default = false;
    for clause in &clauses {
        if clause.kind() == CASE_CLAUSE {
            pending.push(clause.children().next().ok_or("A case without a value")?);
        } else {
            pending_default = true;
        }
        let mut statements = statements_of(clause);
        if statements.is_empty() {
            continue;
        }
        if statements
            .last()
            .is_some_and(|last| last.kind() == BREAK_STATEMENT && last.children().next().is_none())
        {
            statements.pop();
        }
        let [statement] = statements.as_slice() else {
            return Err("A case has more than one statement".to_string());
        };
        let (how, value) = match statement.kind() {
            RETURN_STATEMENT => (
                Use::Return,
                statement.children().next().ok_or("A return without a value")?,
            ),
            ECHO_STATEMENT => {
                let mut operands = statement.children();
                let only = operands.next().ok_or("An echo without a value")?;
                if operands.next().is_some() {
                    return Err("An echo with several values".to_string());
                }
                (Use::Echo, only)
            }
            EXPR_STATEMENT => {
                let assign = statement
                    .children()
                    .next()
                    .filter(|node| node.kind() == ASSIGN_EXPR)
                    .ok_or("A case does more than assign")?;
                if tokens(&assign).all(|token| token.kind() != ASSIGN) {
                    return Err("A case updates a value".to_string());
                }
                let mut parts = assign.children();
                let (target, value) = (parts.next().ok_or("No target")?, parts.next().ok_or("No value")?);
                (Use::Assign(text_of_node(text, &target).to_string()), value)
            }
            _ => return Err("A case does something a match arm cannot".to_string()),
        };
        arms.push(Arm {
            labels: std::mem::take(&mut pending),
            is_default: std::mem::take(&mut pending_default),
            value,
            how,
        });
    }
    if !pending.is_empty() || pending_default {
        return Err("The last case has nothing to do".to_string());
    }
    let how = &arms.first().ok_or("No arm")?.how;
    if arms.iter().any(|arm| arm.how != *how) {
        return Err("The cases do different things".to_string());
    }
    if !arms.iter().any(|arm| arm.is_default) {
        return Err("Without a default, a value nothing matches throws in a match".to_string());
    }
    check_comparisons(rcx, &subject, &arms)?;
    let (unit, eol) = (unit(rcx), eol(text));
    let indent = indent_of(text, start(switch) as usize);
    let mut body = String::new();
    for arm in &arms {
        let labels = if arm.is_default {
            "default".to_string()
        } else {
            arm.labels
                .iter()
                .map(|label| text_of_node(text, label))
                .collect::<Vec<_>>()
                .join(", ")
        };
        body.push_str(&format!(
            "{indent}{unit}{labels} => {},{eol}",
            text_of_node(text, &arm.value)
        ));
    }
    let subject_text = text_of_node(text, &subject);
    let head = format!("match ({subject_text}) {{{eol}{body}{indent}}}");
    Ok(match how {
        Use::Return => format!("return {head};"),
        Use::Echo => format!("echo {head};"),
        Use::Assign(target) => format!("{target} = {head};"),
    })
}

/// A switch compares loosely and a match strictly, so every label has to be the very type of the subject.
fn check_comparisons(rcx: &Rcx<'_>, subject: &SyntaxNode, arms: &[Arm]) -> Result<(), String> {
    let analyzer = rcx.cx.file.analyzer(subject);
    let env = analyzer.env_around(subject);
    let subject_type = analyzer.type_of(subject, &env);
    let kind = match &subject_type {
        Type::Int | Type::IntLiteral(_) => "int",
        Type::String | Type::StringLiteral(_) => "string",
        Type::Class { name, .. }
            if rcx
                .cx
                .index
                .class(name)
                .is_some_and(|class| class.decl.kind == php_index::ClassKind::Enum) =>
        {
            "enum"
        }
        _ => return Err("The type of the subject is not known to be an int, a string or an enum".to_string()),
    };
    for arm in arms {
        for label in &arm.labels {
            let label_type = analyzer.type_of(label, &env);
            let same = match (&label_type, kind) {
                (Type::Int | Type::IntLiteral(_), "int") | (Type::String | Type::StringLiteral(_), "string") => true,
                (Type::Class { name, .. }, "enum") => {
                    matches!(&subject_type, Type::Class { name: own, .. } if own.eq_ignore_ascii_case(name))
                }
                _ => false,
            };
            if !same {
                return Err("A case is not of the type of the subject, which a match compares strictly".to_string());
            }
        }
    }
    Ok(())
}
