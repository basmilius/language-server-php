//! Quick fixes: one per way a finding can be repaired.

use php_index::UseKind;
use php_syntax::TextRange;

use super::edits::{delete, insert, line_end, line_start, remove_with_lines, replace};
use super::imports::{import_candidates, import_one, remove_import};
use super::{Action, ActionInput, ActionKind};
use crate::completion::TextEdit;
use crate::inspections::{Cx, Fix};

fn overlaps(range: TextRange, selection: TextRange) -> bool {
    range.start() <= selection.end() && selection.start() <= range.end()
}

pub(super) fn quick_fixes<'a>(input: &'a ActionInput<'a>, cx: &'a Cx<'a>, out: &mut Vec<Action<'a>>) {
    for (position, finding) in input.findings.iter().enumerate() {
        if !overlaps(finding.diagnostic.range, input.range) {
            continue;
        }
        let range = finding.diagnostic.range;
        match &finding.fix {
            Fix::None => {}
            Fix::Import { name, kind, at } => imports_for(input, cx, position, name, *kind, *at, out),
            Fix::RemoveImport { range: target } => {
                let target = *target;
                out.push(fix(
                    position,
                    "remove-import",
                    "Remove unused import",
                    true,
                    false,
                    Box::new(move || remove_import(cx.text, &cx.root, target).into_iter().collect()),
                    range,
                ));
            }
            Fix::RemoveDeclaration { range: target } => {
                let target = *target;
                out.push(fix(
                    position,
                    "remove-declaration",
                    "Remove unused declaration",
                    false,
                    false,
                    Box::new(move || vec![remove_with_lines(cx.text, target)]),
                    range,
                ));
            }
            Fix::RemoveAssignment {
                statement,
                prefix,
                pure,
            } => {
                let (statement, prefix, pure) = (*statement, *prefix, *pure);
                let title = if pure {
                    "Remove unused variable"
                } else {
                    "Remove the assignment, keep the call"
                };
                out.push(fix(
                    position,
                    "remove-assignment",
                    title,
                    true,
                    false,
                    Box::new(move || {
                        if pure {
                            vec![remove_with_lines(cx.text, statement)]
                        } else {
                            vec![delete(prefix)]
                        }
                    }),
                    range,
                ));
            }
            Fix::RemoveRange { range: target } => {
                let target = *target;
                out.push(fix(
                    position,
                    "remove-unreachable",
                    "Remove unreachable code",
                    true,
                    false,
                    Box::new(move || vec![remove_with_lines(cx.text, target)]),
                    range,
                ));
            }
            Fix::ReplaceAssignment { operator } => {
                let operator = *operator;
                for (id, title, text, preferred) in [
                    ("assign-identical", "Replace '=' with '==='", "===", true),
                    ("assign-equal", "Replace '=' with '=='", "==", false),
                ] {
                    out.push(fix(
                        position,
                        id,
                        title,
                        preferred,
                        false,
                        Box::new(move || vec![replace(operator, text)]),
                        range,
                    ));
                }
            }
            Fix::RemoveDocLine { range: target } => {
                let target = *target;
                out.push(fix(
                    position,
                    "remove-doc-line",
                    "Remove the @param tag",
                    true,
                    false,
                    Box::new(move || vec![remove_doc_tag(cx.text, target)]),
                    range,
                ));
            }
            Fix::ImplementMembers { class } => {
                let class = *class;
                out.push(fix(
                    position,
                    "implement-members",
                    "Implement the missing methods",
                    true,
                    true,
                    Box::new(move || super::members::implement_missing(cx, class)),
                    range,
                ));
            }
            Fix::CreateMember {
                class,
                name,
                is_static,
                is_method,
                at,
            } => {
                let (class, name, is_static, is_method, at) =
                    (class.clone(), name.clone(), *is_static, *is_method, *at);
                let title = if is_method {
                    format!("Create method '{name}()'")
                } else {
                    format!("Create property '${name}'")
                };
                out.push(fix(
                    position,
                    &format!("create-member:{name}"),
                    &title,
                    false,
                    true,
                    Box::new(move || super::members::create_member(cx, &class, &name, is_static, is_method, at)),
                    range,
                ));
            }
            Fix::AddStrictTypes => {
                out.push(fix(
                    position,
                    "strict-types",
                    "Add declare(strict_types=1)",
                    true,
                    false,
                    Box::new(move || strict_types_edit(cx)),
                    range,
                ));
            }
            Fix::Visibility { member, to } => {
                let (member, to) = (*member, *to);
                out.push(fix(
                    position,
                    &format!("visibility:{to}"),
                    &format!("Make it {to}"),
                    true,
                    false,
                    Box::new(move || super::members::visibility_edit(cx, member, to)),
                    range,
                ));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn fix<'a>(
    finding: usize,
    id: &str,
    title: &str,
    preferred: bool,
    expensive: bool,
    edits: Box<dyn Fn() -> Vec<TextEdit> + 'a>,
    range: TextRange,
) -> Action<'a> {
    Action {
        id: format!("{id}@{}", u32::from(range.start())),
        title: title.to_string(),
        kind: ActionKind::QuickFix,
        preferred,
        finding: Some(finding),
        expensive,
        edits,
    }
}

fn imports_for<'a>(
    input: &'a ActionInput<'a>,
    cx: &'a Cx<'a>,
    finding: usize,
    name: &str,
    kind: UseKind,
    at: u32,
    out: &mut Vec<Action<'a>>,
) {
    let analyzer = cx.file.analyzer(&node_at(cx, at));
    let namespace = analyzer.resolver.namespace.clone();
    let candidates = import_candidates(input.env.index, name, kind, &namespace);
    let sole = candidates.len() == 1;
    let range = input.findings[finding].diagnostic.range;
    for candidate in candidates {
        let for_edit = candidate.clone();
        let title = match kind {
            UseKind::Class => format!("Import '{candidate}'"),
            UseKind::Function => format!("Import function '{candidate}'"),
            UseKind::Constant => format!("Import constant '{candidate}'"),
        };
        out.push(fix(
            finding,
            &format!("import:{candidate}"),
            &title,
            sole,
            true,
            Box::new(move || import_one(cx.text, &cx.root, at, &for_edit, kind).into_iter().collect()),
            range,
        ));
        if kind == UseKind::Class && !candidate.contains('\\') && !namespace.is_empty() {
            out.push(fix(
                finding,
                &format!("qualify:{candidate}"),
                &format!("Write '\\{candidate}' in full"),
                false,
                false,
                Box::new(move || vec![insert(at, "\\")]),
                range,
            ));
        }
    }
}

fn node_at(cx: &Cx, offset: u32) -> php_syntax::SyntaxNode {
    crate::ast::node_at(&cx.root, offset)
}

/// A `@param` line out of a doc comment, the whole line when nothing else is on it.
fn remove_doc_tag(text: &str, tag: TextRange) -> TextEdit {
    let start = usize::from(tag.start());
    let end = usize::from(tag.end());
    let line_from = line_start(text, start);
    let before = &text[line_from..start];
    let after_end = line_end(text, end);
    let after = text[end..after_end].trim();
    if before.trim().trim_start_matches('*').trim().is_empty() && after.is_empty() {
        return TextEdit {
            start: line_from as u32,
            end: after_end as u32,
            new_text: String::new(),
        };
    }
    let trailing = text[end..].len() - text[end..].trim_start_matches(' ').len();
    TextEdit {
        start: start as u32,
        end: (end + trailing) as u32,
        new_text: String::new(),
    }
}

fn strict_types_edit(cx: &Cx) -> Vec<TextEdit> {
    let Some(open) = cx
        .root
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == php_syntax::SyntaxKind::OPEN_TAG)
    else {
        return Vec::new();
    };
    let at = u32::from(open.text_range().end());
    let rest = &cx.text[at as usize..];
    let blank_follows = rest.starts_with('\n') || rest.starts_with("\r\n");
    let text = if blank_follows {
        "\ndeclare(strict_types=1);\n"
    } else {
        "\ndeclare(strict_types=1);\n\n"
    };
    vec![insert(at, text)]
}
