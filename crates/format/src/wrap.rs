//! What a line that is too long may be broken at: the lists of arguments and parameters, array
//! items, the links of a method chain, the operands of a long expression and the two sides of a
//! ternary. A construct is only broken when it sits on one line, and every break is a gap that is
//! turned into a line break, so the tokens stay.

use std::collections::HashSet;

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode};

use crate::layout::Layout;
use crate::model::Model;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Wrap {
    List,
    Array,
    Chain,
    Binary,
    Ternary,
}

/// The constructs whose gaps are line breaks, by the offset the construct starts at.
pub(crate) type Forced = HashSet<(Wrap, usize)>;

fn offset_of(node: &SyntaxNode) -> usize {
    usize::from(node.text_range().start())
}

/// The operators that may start a line of a long expression.
fn is_wrappable_operator(kind: SyntaxKind) -> bool {
    matches!(kind, DOT | AND_AND | OR_OR | AND_KW | OR_KW | COALESCE | PLUS | MINUS)
}

/// The operator token of a binary expression.
fn operator_of(binary: &SyntaxNode) -> Option<(SyntaxKind, usize)> {
    binary
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| !token.kind().is_trivia())
        .map(|token| (token.kind(), usize::from(token.text_range().start())))
}

/// A run of binary expressions with the same operator, such as `a . b . c`.
pub(crate) struct Operands {
    pub top: SyntaxNode,
}

pub(crate) fn operands_of(binary: &SyntaxNode) -> Option<Operands> {
    let (kind, _) = operator_of(binary)?;
    if !is_wrappable_operator(kind) {
        return None;
    }
    let same =
        |node: &SyntaxNode| node.kind() == BINARY_EXPR && operator_of(node).is_some_and(|(other, _)| other == kind);
    let mut top = binary.clone();
    while let Some(parent) = top.parent() {
        if same(&parent) && parent.children().next().as_ref() == Some(&top) {
            top = parent;
        } else {
            break;
        }
    }
    Some(Operands { top })
}

/// A method chain: the links are the `->` of the calls and fetches along it.
pub(crate) struct Chain {
    pub top: SyntaxNode,
    /// The offsets of the arrows, in the order of the text, that may start a line.
    pub breaks: Vec<usize>,
}

fn arrow_offset(fetch: &SyntaxNode) -> Option<usize> {
    fetch
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| matches!(token.kind(), ARROW | NULLSAFE_ARROW))
        .map(|token| usize::from(token.text_range().start()))
}

pub(crate) fn chain_of(fetch: &SyntaxNode) -> Option<Chain> {
    let mut top = fetch.clone();
    loop {
        let mut changed = false;
        if let Some(call) = top
            .parent()
            .filter(|parent| parent.kind() == CALL_EXPR && parent.children().next().as_ref() == Some(&top))
        {
            top = call;
            changed = true;
        }
        if let Some(outer) = top
            .parent()
            .filter(|parent| parent.kind() == PROPERTY_FETCH_EXPR && parent.children().next().as_ref() == Some(&top))
        {
            top = outer;
            changed = true;
        }
        if !changed {
            break;
        }
    }
    let mut links: Vec<SyntaxNode> = Vec::new();
    let mut calls = 0;
    let mut current = Some(top.clone());
    let mut base: Option<SyntaxNode> = None;
    while let Some(node) = current {
        match node.kind() {
            CALL_EXPR => {
                if node
                    .children()
                    .next()
                    .is_some_and(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
                {
                    calls += 1;
                }
                current = node.children().next();
            }
            PROPERTY_FETCH_EXPR => {
                links.push(node.clone());
                current = node.children().next();
            }
            INDEX_EXPR => current = node.children().next(),
            _ => {
                base = Some(node);
                current = None;
            }
        }
    }
    links.reverse();
    if links.len() < 2 || calls == 0 || calls < 2 && links.len() < 3 {
        return None;
    }
    let simple_base = base
        .as_ref()
        .is_some_and(|base| matches!(base.kind(), VARIABLE_EXPR | NAME));
    let skip = usize::from(simple_base);
    let breaks: Vec<usize> = links.iter().skip(skip).filter_map(arrow_offset).collect();
    (!breaks.is_empty()).then_some(Chain { top, breaks })
}

/// Whether the gap before the token at `n` is one that a forced construct turns into a line break.
pub(crate) fn broken_before(m: &Model, forced: &Forced, n: usize) -> bool {
    if forced.is_empty() {
        return false;
    }
    let leaf = m.leaf(n);
    let kind = m.kind(n);
    let parent = &leaf.parent;
    match (kind, parent.kind()) {
        (ARROW | NULLSAFE_ARROW, PROPERTY_FETCH_EXPR) => chain_of(parent).is_some_and(|chain| {
            forced.contains(&(Wrap::Chain, offset_of(&chain.top))) && chain.breaks.contains(&leaf.start)
        }),
        (QUESTION | COLON, TERNARY_EXPR) => {
            let key = (Wrap::Ternary, offset_of(parent));
            forced.contains(&key) && (kind == QUESTION || parent.children().count() >= 3)
        }
        (RPAREN | RBRACKET, ARRAY_EXPR) => forced.contains(&(Wrap::Array, offset_of(parent))),
        (RPAREN, IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT) => parent
            .children()
            .next()
            .filter(|condition| condition.kind() == BINARY_EXPR)
            .and_then(|condition| operands_of(&condition))
            .is_some_and(|operands| forced.contains(&(Wrap::Binary, offset_of(&operands.top)))),
        _ => {
            if parent.kind() == BINARY_EXPR {
                if let Some(operands) = operands_of(parent) {
                    if operator_of(parent).is_some_and(|(_, at)| at == leaf.start)
                        && forced.contains(&(Wrap::Binary, offset_of(&operands.top)))
                    {
                        return true;
                    }
                }
            }
            first_of_forced_condition(m, forced, n)
                || m.starts[n].iter().any(|node| {
                    node.kind() == ARRAY_ITEM
                        && node
                            .parent()
                            .is_some_and(|array| forced.contains(&(Wrap::Array, offset_of(&array))))
                })
        }
    }
}

/// The first token inside the parentheses of a condition that is broken over lines.
fn first_of_forced_condition(m: &Model, forced: &Forced, n: usize) -> bool {
    if n == 0 || m.kind(n - 1) != LPAREN {
        return false;
    }
    let opener = &m.leaf(n - 1).parent;
    if !matches!(opener.kind(), IF_STATEMENT | ELSEIF_CLAUSE | WHILE_STATEMENT) {
        return false;
    }
    opener
        .children()
        .next()
        .filter(|condition| condition.kind() == BINARY_EXPR)
        .and_then(|condition| operands_of(&condition))
        .is_some_and(|operands| forced.contains(&(Wrap::Binary, offset_of(&operands.top))))
}

/// What a line that is too long can be broken at, the outermost construct first.
pub(crate) fn choose(
    m: &Model,
    layout: &Layout,
    forced: &Forced,
    first: usize,
    last: usize,
    limit: usize,
) -> Option<(Wrap, usize)> {
    let mut seen: HashSet<(SyntaxKind, usize, usize)> = HashSet::new();
    let mut best: Option<(usize, u8, (Wrap, usize))> = None;
    for position in first..=last {
        for node in m.leaf(position).parent.ancestors() {
            if !seen.insert((node.kind(), offset_of(&node), usize::from(node.text_range().end()))) {
                continue;
            }
            let Some((wrap, key)) = construct(&node) else {
                continue;
            };
            if forced.contains(&(wrap, key)) {
                continue;
            }
            let from = m.sig.partition_point(|&leaf| m.leaves[leaf].start < offset_of(&node));
            let to = m
                .sig
                .partition_point(|&leaf| m.leaves[leaf].end <= usize::from(node.text_range().end()));
            let Some(to) = to.checked_sub(1) else {
                continue;
            };
            if from < first || to > last || from > to {
                continue;
            }
            let column = layout.column[from];
            if column >= limit || layout.end_column[to] <= limit {
                continue;
            }
            let priority = match wrap {
                Wrap::Binary => 0,
                Wrap::Ternary => 1,
                Wrap::Chain => 2,
                Wrap::Array => 3,
                Wrap::List => 4,
            };
            if best.is_none_or(|(held, held_priority, _)| (column, priority) < (held, held_priority)) {
                best = Some((column, priority, (wrap, key)));
            }
        }
    }
    best.map(|(_, _, found)| found)
}

/// Which wrap a node is the start of.
fn construct(node: &SyntaxNode) -> Option<(Wrap, usize)> {
    match node.kind() {
        ARGUMENT_LIST | PARAMETER_LIST => {
            let has_items = node
                .children()
                .any(|child| matches!(child.kind(), ARGUMENT | PARAMETER));
            has_items.then(|| (Wrap::List, offset_of(node)))
        }
        ARRAY_EXPR => node
            .children()
            .any(|child| child.kind() == ARRAY_ITEM)
            .then(|| (Wrap::Array, offset_of(node))),
        BINARY_EXPR => {
            let operands = operands_of(node)?;
            (operands.top == *node).then(|| (Wrap::Binary, offset_of(&operands.top)))
        }
        TERNARY_EXPR => Some((Wrap::Ternary, offset_of(node))),
        PROPERTY_FETCH_EXPR | CALL_EXPR => {
            let fetch = if node.kind() == CALL_EXPR {
                node.children().next()?
            } else {
                node.clone()
            };
            if fetch.kind() != PROPERTY_FETCH_EXPR {
                return None;
            }
            let chain = chain_of(&fetch)?;
            (chain.top == *node).then(|| (Wrap::Chain, offset_of(&chain.top)))
        }
        _ => None,
    }
}
