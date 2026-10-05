//! Completion inside the strings that name a test: data providers, dependencies, functions to
//! cover, groups and Pest datasets.

use php_index::{Index, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::strings::{Role, StringTarget, TestString, string_at};
use super::{is_test_method, test_case_ancestor};
use crate::completion::{CompletionItem, CompletionList, CompletionOptions, ItemKind, TextEdit, match_score};
use crate::infer::Analyzer;

/// The completions for the string around an offset, `None` when the offset is not in a string of a
/// test that names something.
pub fn complete_string(
    index: &Index,
    root: &SyntaxNode,
    text: &str,
    offset: u32,
    options: CompletionOptions,
) -> Option<CompletionList> {
    let analyzer = Analyzer::new(index, root, offset);
    let string = string_at(&analyzer, offset)?;
    let start = u32::from(string.range.start());
    let typed = text.get(start as usize..offset as usize)?;
    let candidates = candidates(&analyzer, root, &string);
    let mut items: Vec<(u8, CompletionItem)> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let score = match_score(&candidate.name, typed)?;
            Some((
                score,
                CompletionItem {
                    label: candidate.name.clone(),
                    kind: candidate.kind,
                    detail: candidate.detail,
                    description: candidate.description,
                    edit: TextEdit {
                        start,
                        end: u32::from(string.range.end()),
                        new_text: candidate.name.clone(),
                    },
                    additional_edits: Vec::new(),
                    sort_text: format!("{}{}", candidate.rank, candidate.name.to_ascii_lowercase()),
                    filter_text: Some(candidate.name),
                    deprecated: false,
                    data: None,
                },
            ))
        })
        .collect();
    items.sort_by(|left, right| (left.0, &left.1.sort_text).cmp(&(right.0, &right.1.sort_text)));
    let incomplete = items.len() > options.limit;
    items.truncate(options.limit);
    Some(CompletionList {
        items: items.into_iter().map(|(_, item)| item).collect(),
        incomplete,
    })
}

pub struct Candidate {
    pub name: String,
    pub kind: ItemKind,
    pub detail: Option<String>,
    pub description: Option<String>,
    /// Sorts the candidates that fit best first.
    pub rank: u8,
}

fn candidates(analyzer: &Analyzer<'_>, root: &SyntaxNode, string: &TestString) -> Vec<Candidate> {
    let index = analyzer.index;
    match &string.target {
        StringTarget::Method { class, role } => method_candidates(analyzer, root, string, class, *role),
        StringTarget::Function => index
            .function_names()
            .filter(|name| name.summary.availability.contains(index.level))
            .map(|name| Candidate {
                name: name.summary.name.clone(),
                kind: ItemKind::Function,
                detail: None,
                description: None,
                rank: 0,
            })
            .collect(),
        StringTarget::Group => groups(index, root)
            .into_iter()
            .map(|name| Candidate {
                name,
                kind: ItemKind::Keyword,
                detail: None,
                description: Some("group".to_string()),
                rank: 0,
            })
            .collect(),
        StringTarget::Dataset if string.declaration => Vec::new(),
        StringTarget::Dataset => crate::pest::dataset_candidates(index, root),
    }
}

fn method_candidates(
    analyzer: &Analyzer<'_>,
    root: &SyntaxNode,
    string: &TestString,
    class: &str,
    role: Role,
) -> Vec<Candidate> {
    let index = analyzer.index;
    let level = index.level;
    let own = enclosing_method(root, u32::from(string.range.start()));
    let in_test_case = test_case_ancestor(index, class).is_some();
    let mut out = Vec::new();
    for found in index.methods(&Type::class(class.to_string())) {
        let method = &found.member;
        let name = &method.name;
        if method.visibility != php_index::Visibility::Public
            || found.class.decl.name.starts_with("PHPUnit\\")
            || name.starts_with("__")
            || own.as_deref() == Some(name.as_str())
        {
            continue;
        }
        let is_test = in_test_case && is_test_method(found.class.decl, method);
        let rank = match role {
            Role::DataProvider if is_test || is_lifecycle(name) => continue,
            Role::DataProvider => u8::from(!method.is_static),
            Role::Depends if !is_test => continue,
            Role::Mocked if method.is_final || method.is_static => continue,
            _ => 0,
        };
        out.push(Candidate {
            name: name.clone(),
            kind: ItemKind::Method,
            detail: Some(crate::render::callable_text(&method.callable, level)),
            description: Some(crate::short(&found.class.decl.name).to_string()),
            rank,
        });
    }
    out
}

fn is_lifecycle(name: &str) -> bool {
    matches!(
        name,
        "setUp"
            | "tearDown"
            | "setUpBeforeClass"
            | "tearDownAfterClass"
            | "assertPreConditions"
            | "assertPostConditions"
    )
}

/// The name of the method an attribute or tag belongs to.
fn enclosing_method(root: &SyntaxNode, offset: u32) -> Option<String> {
    crate::ast::node_at(root, offset)
        .ancestors()
        .find(|node| node.kind() == METHOD_DECLARATION)
        .and_then(|method| crate::ast::child_of(&method, NAME))
        .map(|name| crate::ast::text_of(&name))
}

/// Every group the project's tests use, and the ones of the file being edited.
fn groups(index: &Index, root: &SyntaxNode) -> Vec<String> {
    let mut groups: Vec<String> = index
        .test_files()
        .flat_map(|(_, facts)| facts.groups.iter().cloned())
        .collect();
    let mut own = php_index::test_facts::TestFacts::default();
    let resolver = php_index::extract::resolver_at(root, u32::from(root.text_range().end()));
    for class in root
        .descendants()
        .filter(|node| matches!(node.kind(), CLASS_DECLARATION))
    {
        php_index::test_facts::read_groups(&class, &resolver, &mut own);
    }
    groups.extend(own.groups);
    groups.retain(|group| !group.is_empty());
    groups.sort();
    groups.dedup();
    groups
}

/// Whether a token is in an argument of an attribute that takes a class, `#[CoversClass(...)]` and
/// its kin, where a class is written as `Foo::class`.
pub fn in_class_argument(analyzer: &Analyzer<'_>, token: &php_syntax::SyntaxToken) -> bool {
    let mut inside_argument = false;
    for ancestor in token.parent_ancestors() {
        match ancestor.kind() {
            ARGUMENT => inside_argument = true,
            ATTRIBUTE if inside_argument => {
                let Some(name) = crate::ast::child_of(&ancestor, NAME) else {
                    return false;
                };
                let resolved = analyzer.resolver.resolve_class(&crate::ast::text_of(&name));
                return resolved.strip_prefix(super::ATTRIBUTES).is_some_and(|short| {
                    ["CoversClass", "UsesClass", "CoversTrait", "UsesTrait", "DependsOnClass"]
                        .iter()
                        .any(|wanted| wanted.eq_ignore_ascii_case(short))
                });
            }
            ATTRIBUTE => return false,
            _ => {}
        }
    }
    false
}
