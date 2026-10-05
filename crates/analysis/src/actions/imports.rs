//! Imports: finding what a name could be imported from, removing what nothing uses and putting
//! the rest in order.

use std::collections::HashSet;

use php_index::{Index, NameResolver, UseKind};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use super::edits::{delete, line_end, line_start, remove_with_lines, replace};
use crate::ast::{child_of, has_token, text_of};
use crate::completion::TextEdit;
use crate::imports::{ImportPlan, import_edit, plan_import};
use crate::inspections::Cx;
use crate::inspections::unused::{ImportClause, import_clauses};

/// The qualified names a short name may stand for, in the order to offer them: the ones that share
/// the file's root namespace first, then the rest alphabetically.
pub fn import_candidates(index: &Index, name: &str, kind: UseKind, namespace: &str) -> Vec<String> {
    let mut found: Vec<String> = match kind {
        UseKind::Class => index
            .class_names()
            .filter(|class| crate::short(&class.summary.name).eq_ignore_ascii_case(name))
            .map(|class| class.summary.name.clone())
            .collect(),
        UseKind::Function => index
            .function_names()
            .filter(|function| crate::short(&function.summary.name).eq_ignore_ascii_case(name))
            .map(|function| function.summary.name.clone())
            .collect(),
        UseKind::Constant => index
            .constant_names()
            .filter(|constant| crate::short(&constant.summary.name) == name)
            .map(|constant| constant.summary.name.clone())
            .collect(),
    };
    found.sort();
    found.dedup();
    let root = namespace.split('\\').next().unwrap_or("");
    found.sort_by_key(|candidate| {
        let shares = !root.is_empty() && candidate.split('\\').next() == Some(root);
        (!shares, candidate.clone())
    });
    found.truncate(8);
    found
}

/// Writes classes the way they are best written at a place: by short name with an import, by the
/// name as is where the namespace has it, or in full where the short name is taken.
pub struct ClassWriter<'a> {
    index: &'a Index,
    resolver: &'a NameResolver,
    text: &'a str,
    root: &'a SyntaxNode,
    offset: u32,
    imports: Vec<TextEdit>,
}

impl<'a> ClassWriter<'a> {
    pub fn new(
        index: &'a Index,
        resolver: &'a NameResolver,
        text: &'a str,
        root: &'a SyntaxNode,
        offset: u32,
    ) -> ClassWriter<'a> {
        ClassWriter {
            index,
            resolver,
            text,
            root,
            offset,
            imports: Vec::new(),
        }
    }

    /// The alias a file gives a class in an import of its own, as it is written there.
    fn alias_of(&self, fqn: &str) -> Option<String> {
        let wanted = fqn.trim_start_matches('\\');
        self.root
            .descendants()
            .filter(|node| node.kind() == USE_CLAUSE && has_token(node, AS_KW))
            .filter(|clause| {
                child_of(clause, NAME)
                    .is_some_and(|name| text_of(&name).trim_start_matches('\\').eq_ignore_ascii_case(wanted))
            })
            .find_map(|clause| {
                let after = clause
                    .descendants_with_tokens()
                    .filter_map(php_syntax::SyntaxElement::into_token)
                    .skip_while(|token| token.kind() != AS_KW)
                    .find(|token| token.kind() == IDENT)?;
                Some(after.text().to_string())
            })
            .filter(|alias| {
                self.resolver
                    .imports(UseKind::Class)
                    .any(|(known, _)| known.eq_ignore_ascii_case(alias))
            })
    }

    pub fn written(&mut self, fqn: &str) -> String {
        if let Some(alias) = self.alias_of(fqn) {
            return alias;
        }
        let plan = plan_import(self.resolver, fqn, UseKind::Class, |name| {
            let own = self.resolver.qualify(name);
            self.index
                .class(&own)
                .is_some_and(|other| !other.decl.name.eq_ignore_ascii_case(fqn))
        });
        match plan {
            ImportPlan::Plain(name) => name,
            ImportPlan::Qualified(name) => format!("\\{name}"),
            ImportPlan::Import(name) => {
                if let Some(edit) = import_edit(self.text, self.root, self.offset, fqn, UseKind::Class) {
                    if !self.imports.contains(&edit) {
                        self.imports.push(edit);
                    }
                }
                name
            }
        }
    }

    /// The `use` edits the names written so far need.
    pub fn into_imports(self) -> Vec<TextEdit> {
        super::edits::merge_inserts(self.imports)
    }
}

/// The edit that imports one name, or `None` when the file has it already.
pub fn import_one(text: &str, root: &SyntaxNode, at: u32, fqn: &str, kind: UseKind) -> Option<TextEdit> {
    import_edit(text, root, at, fqn, kind)
}

/// Takes an unused import out: the statement, or the clause and a comma beside it.
pub fn remove_import(text: &str, root: &SyntaxNode, range: TextRange) -> Option<TextEdit> {
    let node = root
        .covering_element(range)
        .ancestors_with_self()
        .find(|candidate| candidate.text_range() == range && matches!(candidate.kind(), USE_STATEMENT | USE_CLAUSE))?;
    if node.kind() == USE_STATEMENT {
        return Some(remove_with_lines(text, range));
    }
    let siblings: Vec<SyntaxNode> = node
        .parent()?
        .children()
        .filter(|sibling| sibling.kind() == USE_CLAUSE)
        .collect();
    let position = siblings.iter().position(|sibling| *sibling == node)?;
    if siblings.len() == 1 {
        return Some(remove_with_lines(text, node.parent()?.parent()?.text_range()));
    }
    Some(match siblings.get(position + 1) {
        Some(next) => delete(TextRange::new(node.text_range().start(), next.text_range().start())),
        None => {
            let previous = &siblings[position - 1];
            delete(TextRange::new(previous.text_range().end(), node.text_range().end()))
        }
    })
}

trait AncestorsWithSelf {
    fn ancestors_with_self(&self) -> Box<dyn Iterator<Item = SyntaxNode>>;
}

impl AncestorsWithSelf for php_syntax::SyntaxElement {
    fn ancestors_with_self(&self) -> Box<dyn Iterator<Item = SyntaxNode>> {
        match self {
            php_syntax::SyntaxElement::Node(node) => Box::new(node.ancestors()),
            php_syntax::SyntaxElement::Token(token) => match token.parent() {
                Some(parent) => Box::new(parent.ancestors()),
                None => Box::new(std::iter::empty()),
            },
        }
    }
}

/// One `use` statement as it will be written, and how it sorts.
struct Statement {
    kind: UseKind,
    key: String,
    text: String,
}

/// The edits that remove unused imports, put each kind in its own block and sort the blocks, or
/// `None` when there is nothing to change or something between the statements is not a statement.
pub fn organize_imports(cx: &Cx) -> Vec<TextEdit> {
    let clauses = import_clauses(cx);
    let mut containers: Vec<SyntaxNode> = Vec::new();
    for entry in &clauses {
        if let Some(parent) = entry.statement.parent() {
            if !containers.contains(&parent) {
                containers.push(parent);
            }
        }
    }
    containers
        .iter()
        .filter_map(|container| organize_container(cx, container, &clauses))
        .collect()
}

fn organize_container(cx: &Cx, container: &SyntaxNode, clauses: &[ImportClause]) -> Option<TextEdit> {
    let statements: Vec<SyntaxNode> = container
        .children()
        .filter(|child| child.kind() == USE_STATEMENT)
        .collect();
    let (first, last) = (statements.first()?, statements.last()?);
    let text = cx.text;
    let region_start = line_start(text, usize::from(first.text_range().start()));
    let region_end = line_end(text, usize::from(last.text_range().end()));
    // Comments or code among the statements stay where they are.
    let between = &text[usize::from(first.text_range().start())..usize::from(last.text_range().end())];
    let mut inner_text = between.to_string();
    for statement in &statements {
        let from = usize::from(statement.text_range().start()) - usize::from(first.text_range().start());
        let to = usize::from(statement.text_range().end()) - usize::from(first.text_range().start());
        inner_text.replace_range(from..to, &" ".repeat(to - from));
    }
    if !inner_text.trim().is_empty() {
        return None;
    }
    let mut written: Vec<Statement> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for statement in &statements {
        let own: Vec<&ImportClause> = clauses.iter().filter(|entry| entry.statement == *statement).collect();
        let kept: Vec<&ImportClause> = own.iter().copied().filter(|entry| entry.used).collect();
        if kept.is_empty() {
            continue;
        }
        let rendered = render_statement(statement, &kept)?;
        if seen.insert(format!("{}:{}", rendered.kind as u8, rendered.text)) {
            written.push(rendered);
        }
    }
    written.sort_by(|left, right| (kind_order(left.kind), &left.key).cmp(&(kind_order(right.kind), &right.key)));
    let mut out = String::new();
    let mut previous: Option<UseKind> = None;
    for statement in &written {
        if previous.is_some_and(|kind| kind != statement.kind) {
            out.push('\n');
        }
        out.push_str(&statement.text);
        out.push('\n');
        previous = Some(statement.kind);
    }
    let original = &text[region_start..region_end];
    let mut normalized = out.clone();
    if !original.ends_with('\n') && !original.is_empty() {
        normalized.pop();
    }
    if normalized == original {
        return None;
    }
    Some(replace(
        TextRange::new((region_start as u32).into(), (region_end as u32).into()),
        normalized,
    ))
}

fn kind_order(kind: UseKind) -> u8 {
    match kind {
        UseKind::Class => 0,
        UseKind::Function => 1,
        UseKind::Constant => 2,
    }
}

fn render_statement(statement: &SyntaxNode, kept: &[&ImportClause]) -> Option<Statement> {
    let kind = kept[0].kind;
    let keyword = match kind {
        UseKind::Class => "use ",
        UseKind::Function => "use function ",
        UseKind::Constant => "use const ",
    };
    if let Some(group) = child_of(statement, USE_GROUP) {
        let prefix = child_of(&group, NAME).map(|name| text_of(&name))?;
        let mut parts: Vec<(String, String)> = kept
            .iter()
            .map(|entry| {
                let text = text_of(&entry.clause);
                (text.to_ascii_lowercase(), text)
            })
            .collect();
        parts.sort();
        let names: Vec<String> = parts.into_iter().map(|(_, text)| text).collect();
        let prefix = prefix.trim_end_matches('\\').to_string();
        return Some(Statement {
            kind,
            key: prefix.trim_start_matches('\\').to_ascii_lowercase(),
            text: format!("{keyword}{prefix}\\{{{}}};", names.join(", ")),
        });
    }
    let clauses: Vec<String> = kept.iter().map(|entry| text_of(&entry.clause)).collect();
    let key = kept[0].full.to_ascii_lowercase();
    Some(Statement {
        kind,
        key,
        text: format!("{keyword}{};", clauses.join(", ")),
    })
}
