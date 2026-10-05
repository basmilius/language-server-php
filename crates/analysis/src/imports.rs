//! Where a `use` statement goes: in the right block, in sorted order, in the style of the file.

use php_index::{NameResolver, UseKind};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use crate::ast::{child_of, end, has_token, start, text_of};
use crate::completion::TextEdit;

/// How a class is written where it is used, and what has to be added to the file for that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportPlan {
    /// The name as it is; nothing to add.
    Plain(String),
    /// The fully qualified name with a leading backslash, because the short name is taken.
    Qualified(String),
    /// The short name, with a `use` statement added.
    Import(String),
}

/// Decides how to write the class `fqn` at a point whose names `resolver` resolves.
pub fn plan_import(
    resolver: &NameResolver,
    fqn: &str,
    kind: UseKind,
    taken_in_namespace: impl Fn(&str) -> bool,
) -> ImportPlan {
    let short = fqn.rsplit('\\').next().unwrap_or(fqn).to_string();
    let namespace = fqn.rsplit_once('\\').map_or("", |(namespace, _)| namespace);
    let imported = resolver
        .imports(kind)
        .find(|(alias, target)| {
            let same_alias = match kind {
                UseKind::Constant => *alias == short,
                _ => alias.eq_ignore_ascii_case(&short),
            };
            same_alias && !target.eq_ignore_ascii_case(fqn)
        })
        .is_some();
    let already = resolver
        .imports(kind)
        .any(|(alias, target)| target.eq_ignore_ascii_case(fqn) && alias.eq_ignore_ascii_case(&short));
    if already {
        return ImportPlan::Plain(short);
    }
    if namespace.eq_ignore_ascii_case(&resolver.namespace) {
        return if imported {
            ImportPlan::Qualified(fqn.to_string())
        } else {
            ImportPlan::Plain(short)
        };
    }
    if imported {
        return ImportPlan::Qualified(fqn.to_string());
    }
    if kind != UseKind::Class && namespace.is_empty() {
        // Global functions and constants fall back from any namespace.
        return ImportPlan::Plain(short);
    }
    if taken_in_namespace(&short) {
        return ImportPlan::Qualified(fqn.to_string());
    }
    ImportPlan::Import(short)
}

struct UseEntry {
    key: String,
    /// The start of the line the statement is on, and the end of its line including the break.
    line_start: usize,
    line_end: usize,
}

fn line_bounds(text: &str, range: TextRange) -> (usize, usize) {
    let from = usize::from(range.start());
    let to = usize::from(range.end());
    let line_start = text[..from].rfind('\n').map_or(0, |index| index + 1);
    let line_end = if text[..to].ends_with('\n') {
        to
    } else {
        text[to..].find('\n').map_or(text.len(), |index| to + index + 1)
    };
    (line_start, line_end)
}

fn statement_kind(node: &SyntaxNode) -> UseKind {
    if has_token(node, FUNCTION_KW) {
        UseKind::Function
    } else if has_token(node, CONST_KW) {
        UseKind::Constant
    } else {
        UseKind::Class
    }
}

/// The container whose statements hold the `use` lines for an offset: the namespace around it, or the file.
fn use_container(root: &SyntaxNode, offset: u32) -> SyntaxNode {
    let mut container = root.clone();
    for node in root.descendants().filter(|node| node.kind() == NAMESPACE_DECLARATION) {
        let inside = start(&node) <= offset && offset <= end(&node);
        let unbraced = child_of(&node, BLOCK).is_none();
        if inside || (unbraced && start(&node) <= offset) {
            container = match child_of(&node, BLOCK).or_else(|| child_of(&node, STATEMENT_LIST)) {
                Some(body) => body,
                None => node,
            };
        }
    }
    container
}

/// The edit that imports `fqn`, or `None` when the text already has it.
pub fn import_edit(text: &str, root: &SyntaxNode, offset: u32, fqn: &str, kind: UseKind) -> Option<TextEdit> {
    let container = use_container(root, offset);
    let statements: Vec<SyntaxNode> = container
        .children()
        .filter(|node| node.kind() == USE_STATEMENT)
        .collect();
    let line = |fqn: &str| match kind {
        UseKind::Class => format!("use {fqn};\n"),
        UseKind::Function => format!("use function {fqn};\n"),
        UseKind::Constant => format!("use const {fqn};\n"),
    };
    let new_key = fqn.to_ascii_lowercase();

    let mut entries: Vec<UseEntry> = Vec::new();
    for statement in &statements {
        if statement_kind(statement) != kind {
            continue;
        }
        let (line_start, line_end) = line_bounds(text, statement.text_range());
        let key = match child_of(statement, USE_GROUP) {
            Some(group) => child_of(&group, NAME).map(|name| text_of(&name)).unwrap_or_default(),
            None => statement
                .children()
                .find(|clause| clause.kind() == USE_CLAUSE)
                .and_then(|clause| child_of(&clause, NAME))
                .map(|name| text_of(&name))
                .unwrap_or_default(),
        };
        entries.push(UseEntry {
            key: key.trim_start_matches('\\').to_ascii_lowercase(),
            line_start,
            line_end,
        });
    }

    if !entries.is_empty() {
        let mut groups: Vec<Vec<&UseEntry>> = Vec::new();
        for entry in &entries {
            let starts_new_group = match groups.last().and_then(|group| group.last()) {
                Some(previous) => text[previous.line_end.min(entry.line_start)..entry.line_start].contains('\n'),
                None => true,
            };
            if starts_new_group {
                groups.push(vec![entry]);
            } else if let Some(group) = groups.last_mut() {
                group.push(entry);
            }
        }
        let root_segment = new_key.split('\\').next().unwrap_or("");
        let group = groups
            .iter()
            .find(|group| {
                group
                    .iter()
                    .any(|entry| entry.key.split('\\').next() == Some(root_segment))
            })
            .or_else(|| {
                groups
                    .iter()
                    .rev()
                    .find(|group| group[0].key.as_str() <= new_key.as_str())
            })
            .unwrap_or(&groups[0]);
        if group.iter().any(|entry| entry.key == new_key) {
            return None;
        }
        let position = match group.iter().find(|entry| entry.key.as_str() > new_key.as_str()) {
            Some(next) => next.line_start,
            None => group[group.len() - 1].line_end,
        };
        let mut new_text = line(fqn);
        if position == text.len() && !text[..position].ends_with('\n') {
            new_text.insert(0, '\n');
        }
        return Some(TextEdit {
            start: position as u32,
            end: position as u32,
            new_text,
        });
    }

    // No statement of this kind: after the last `use` of any kind, else after the namespace or the opening.
    if let Some(last) = statements.last() {
        let (_, line_end) = line_bounds(text, last.text_range());
        return Some(TextEdit {
            start: line_end as u32,
            end: line_end as u32,
            new_text: format!("\n{}", line(fqn)),
        });
    }
    let anchor = anchor_end(text, root, &container)?;
    let rest = &text[anchor..];
    let blank_follows = rest.starts_with('\n') || rest.starts_with("\r\n") || rest.is_empty();
    let new_text = if blank_follows {
        format!("\n{}", line(fqn))
    } else {
        format!("\n{}\n", line(fqn))
    };
    Some(TextEdit {
        start: anchor as u32,
        end: anchor as u32,
        new_text,
    })
}

/// The end of the line after which the first `use` goes: after the namespace, else after `declare`, else after the opening tag.
fn anchor_end(text: &str, root: &SyntaxNode, container: &SyntaxNode) -> Option<usize> {
    if container.kind() == BLOCK {
        let brace = container.first_token()?;
        let (_, line_end) = line_bounds(text, brace.text_range());
        return Some(line_end);
    }
    if let Some(namespace) = container
        .parent()
        .filter(|parent| parent.kind() == NAMESPACE_DECLARATION)
    {
        let header_end = namespace
            .children_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|token| token.kind() == SEMICOLON)?;
        let (_, line_end) = line_bounds(text, header_end.text_range());
        return Some(line_end);
    }
    let mut anchor = None;
    for child in root.children_with_tokens() {
        match child {
            php_syntax::SyntaxElement::Token(token) if token.kind() == OPEN_TAG => {
                let (_, line_end) = line_bounds(text, token.text_range());
                anchor = Some(line_end);
            }
            php_syntax::SyntaxElement::Node(node) if node.kind() == DECLARE_STATEMENT => {
                let (_, line_end) = line_bounds(text, node.text_range());
                anchor = Some(line_end);
            }
            php_syntax::SyntaxElement::Node(_) => break,
            _ => {}
        }
    }
    anchor
}

#[cfg(test)]
mod tests {
    use super::*;
    use php_syntax::parse;

    fn apply(text: &str, offset: usize, fqn: &str) -> String {
        let root = parse(text).syntax();
        let edit = import_edit(text, &root, offset as u32, fqn, UseKind::Class).expect("an edit");
        let mut out = text.to_string();
        out.replace_range(edit.start as usize..edit.end as usize, &edit.new_text);
        out
    }

    #[test]
    fn inserts_in_sorted_order_within_a_block() {
        let text = "<?php\n\nnamespace App;\n\nuse App\\Alpha;\nuse App\\Gamma;\n\nclass A {}\n";
        assert_eq!(
            apply(text, text.len(), "App\\Beta"),
            "<?php\n\nnamespace App;\n\nuse App\\Alpha;\nuse App\\Beta;\nuse App\\Gamma;\n\nclass A {}\n"
        );
        assert_eq!(
            apply(text, text.len(), "App\\Zed"),
            "<?php\n\nnamespace App;\n\nuse App\\Alpha;\nuse App\\Gamma;\nuse App\\Zed;\n\nclass A {}\n"
        );
    }

    #[test]
    fn picks_the_group_that_shares_the_root_namespace() {
        let text = "<?php\n\nuse Illuminate\\Support\\Str;\n\nuse App\\Models\\User;\n\nclass A {}\n";
        assert_eq!(
            apply(text, text.len(), "App\\Models\\Post"),
            "<?php\n\nuse Illuminate\\Support\\Str;\n\nuse App\\Models\\Post;\nuse App\\Models\\User;\n\nclass A {}\n"
        );
        assert_eq!(
            apply(text, text.len(), "Illuminate\\Http\\Request"),
            "<?php\n\nuse Illuminate\\Http\\Request;\nuse Illuminate\\Support\\Str;\n\nuse App\\Models\\User;\n\nclass A {}\n"
        );
    }

    #[test]
    fn starts_the_first_block_after_the_namespace_or_the_opening_tag() {
        let namespaced = "<?php\n\nnamespace App;\n\nclass A {}\n";
        assert_eq!(
            apply(namespaced, namespaced.len(), "Foo\\Bar"),
            "<?php\n\nnamespace App;\n\nuse Foo\\Bar;\n\nclass A {}\n"
        );
        let plain = "<?php\n\nclass A {}\n";
        assert_eq!(
            apply(plain, plain.len(), "Foo\\Bar"),
            "<?php\n\nuse Foo\\Bar;\n\nclass A {}\n"
        );
        let strict = "<?php\ndeclare(strict_types=1);\n\nclass A {}\n";
        assert_eq!(
            apply(strict, strict.len(), "Foo\\Bar"),
            "<?php\ndeclare(strict_types=1);\n\nuse Foo\\Bar;\n\nclass A {}\n"
        );
    }

    #[test]
    fn leaves_grouped_uses_alone() {
        let text = "<?php\n\nuse Foo\\{A, B};\n\nclass X {}\n";
        assert_eq!(
            apply(text, text.len(), "Foo\\C"),
            "<?php\n\nuse Foo\\{A, B};\nuse Foo\\C;\n\nclass X {}\n"
        );
    }

    #[test]
    fn plans_how_a_class_is_written() {
        let mut resolver = NameResolver::new("App\\Http");
        resolver.add_use(UseKind::Class, "Foo\\Thing", None);
        let free = |_: &str| false;
        assert_eq!(
            plan_import(&resolver, "App\\Http\\Local", UseKind::Class, free),
            ImportPlan::Plain("Local".into())
        );
        assert_eq!(
            plan_import(&resolver, "Foo\\Thing", UseKind::Class, free),
            ImportPlan::Plain("Thing".into())
        );
        assert_eq!(
            plan_import(&resolver, "Bar\\Thing", UseKind::Class, free),
            ImportPlan::Qualified("Bar\\Thing".into())
        );
        assert_eq!(
            plan_import(&resolver, "Bar\\Other", UseKind::Class, free),
            ImportPlan::Import("Other".into())
        );
        assert_eq!(
            plan_import(&resolver, "DateTime", UseKind::Class, free),
            ImportPlan::Import("DateTime".into())
        );
        assert_eq!(
            plan_import(&resolver, "strlen", UseKind::Function, free),
            ImportPlan::Plain("strlen".into())
        );
        assert_eq!(
            plan_import(&resolver, "Bar\\Other", UseKind::Class, |name| name == "Other"),
            ImportPlan::Qualified("Bar\\Other".into())
        );
        let global = NameResolver::new("");
        assert_eq!(
            plan_import(&global, "DateTime", UseKind::Class, free),
            ImportPlan::Plain("DateTime".into())
        );
    }
}
