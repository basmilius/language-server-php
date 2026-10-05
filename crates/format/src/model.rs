//! The tokens of a file in the order of the text, with what the rules need to know about each
//! one: where it sits in the tree, which nodes start and end at it, and whether it begins an item
//! of a list of statements or members.

use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode, WalkEvent, parse};

/// Why a text is left as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The text has syntax errors, so there is no telling what a layout would mean.
    SyntaxErrors,
    /// The text has markup around its PHP, or more than one PHP block.
    Markup,
    /// The layout would have changed a token, which a formatter must never do.
    ChangedTokens,
    /// Nothing but whitespace.
    Empty,
}

/// A token of the tree, trivia included.
pub(crate) struct Leaf {
    pub kind: SyntaxKind,
    pub start: usize,
    pub end: usize,
    pub parent: SyntaxNode,
    /// The start of the outermost string or heredoc around it, whose insides are not touched.
    pub opaque: Option<usize>,
}

/// A statement or member that is not the first of its list.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Boundary {
    pub kind: SyntaxKind,
    pub previous: SyntaxKind,
}

pub(crate) struct Model<'a> {
    pub text: &'a str,
    pub leaves: Vec<Leaf>,
    /// The leaf of each token that is not whitespace or a comment.
    pub sig: Vec<usize>,
    /// The nodes that begin at each significant token, the outermost first.
    pub starts: Vec<Vec<SyntaxNode>>,
    /// How many of the nodes that matter for indentation end at each significant token.
    pub ends: Vec<usize>,
    pub boundaries: Vec<Option<Boundary>>,
}

/// The nodes whose later lines are continuations of their first.
fn is_continuation(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        EXPR_STATEMENT
            | ECHO_STATEMENT
            | IF_STATEMENT
            | WHILE_STATEMENT
            | DO_WHILE_STATEMENT
            | FOR_STATEMENT
            | FOREACH_STATEMENT
            | SWITCH_STATEMENT
            | BREAK_STATEMENT
            | CONTINUE_STATEMENT
            | RETURN_STATEMENT
            | GLOBAL_STATEMENT
            | STATIC_VARIABLE_STATEMENT
            | UNSET_STATEMENT
            | TRY_STATEMENT
            | GOTO_STATEMENT
            | DECLARE_STATEMENT
            | CONST_STATEMENT
            | NAMESPACE_DECLARATION
            | USE_STATEMENT
            | FUNCTION_DECLARATION
            | CLASS_DECLARATION
            | INTERFACE_DECLARATION
            | TRAIT_DECLARATION
            | ENUM_DECLARATION
            | METHOD_DECLARATION
            | PROPERTY_DECLARATION
            | CLASS_CONST_DECLARATION
            | TRAIT_USE
            | ENUM_CASE
            | CASE_CLAUSE
            | DEFAULT_CLAUSE
            | ARGUMENT
            | ARRAY_ITEM
            | MATCH_ARM
            | PARAMETER
            | PROPERTY_HOOK
            | USE_CLAUSE
    )
}

fn is_opaque(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        INTERPOLATED_STRING | HEREDOC | SHELL_EXEC_EXPR | BRACED_INTERPOLATION | DOLLAR_BRACE_INTERPOLATION
    )
}

/// The token a declaration really begins at: the one after the attribute lists that lead it, which
/// sit on lines of their own.
fn after_attributes(node: &SyntaxNode, leaves: &[Leaf], sig: &[usize], first: usize) -> usize {
    let mut end = None;
    for child in node.children() {
        if child.kind() != ATTRIBUTE_LIST {
            break;
        }
        end = Some(usize::from(child.text_range().end()));
    }
    match end {
        Some(end) => sig.partition_point(|&leaf| leaves[leaf].start < end).max(first),
        None => first,
    }
}

/// The children of a node that are items of its list: statements, members, clauses.
fn items_of(node: &SyntaxNode) -> Vec<SyntaxNode> {
    let children: Vec<SyntaxNode> = node.children().collect();
    match node.kind() {
        SOURCE_FILE | BLOCK | STATEMENT_LIST | CLASS_BODY | DEFAULT_CLAUSE => children,
        SWITCH_STATEMENT => children
            .into_iter()
            .filter(|child| matches!(child.kind(), CASE_CLAUSE | DEFAULT_CLAUSE))
            .collect(),
        CASE_CLAUSE => children.into_iter().skip(1).collect(),
        _ => Vec::new(),
    }
}

impl<'a> Model<'a> {
    /// Reads a text, or says why it cannot be laid out by a formatter that only moves whitespace.
    pub(crate) fn build(text: &'a str, allow_errors: bool) -> Result<Model<'a>, Refusal> {
        let parsed = parse(text);
        if !allow_errors && !parsed.errors().is_empty() {
            return Err(Refusal::SyntaxErrors);
        }
        let root = parsed.syntax();
        let mut leaves: Vec<Leaf> = Vec::new();
        let mut sig: Vec<usize> = Vec::new();
        struct Open {
            node: SyntaxNode,
            continuation: bool,
            opaque: bool,
            first: Option<usize>,
            depth: usize,
        }
        let mut open: Vec<Open> = Vec::new();
        let mut starts_raw: Vec<(usize, usize, SyntaxNode)> = Vec::new();
        let mut ends: Vec<usize> = Vec::new();
        let mut boundaries: Vec<Option<Boundary>> = Vec::new();
        let mut opens = 0usize;
        for event in root.preorder_with_tokens() {
            match event {
                WalkEvent::Enter(php_syntax::SyntaxElement::Node(node)) => {
                    open.push(Open {
                        continuation: is_continuation(node.kind()),
                        opaque: is_opaque(node.kind()),
                        node,
                        first: None,
                        depth: open.len(),
                    });
                }
                WalkEvent::Enter(php_syntax::SyntaxElement::Token(token)) => {
                    let kind = token.kind();
                    if kind == INLINE_HTML && !token.text().trim().is_empty() || kind == OPEN_TAG_ECHO {
                        return Err(Refusal::Markup);
                    }
                    if kind == OPEN_TAG {
                        opens += 1;
                        if opens > 1 {
                            return Err(Refusal::Markup);
                        }
                    }
                    let range = token.text_range();
                    let opaque = open
                        .iter()
                        .find(|entry| entry.opaque)
                        .map(|entry| usize::from(entry.node.text_range().start()));
                    let Some(parent) = token.parent() else {
                        return Err(Refusal::SyntaxErrors);
                    };
                    let index = leaves.len();
                    leaves.push(Leaf {
                        kind,
                        start: usize::from(range.start()),
                        end: usize::from(range.end()),
                        parent,
                        opaque,
                    });
                    if !kind.is_trivia() {
                        let position = sig.len();
                        sig.push(index);
                        ends.push(0);
                        boundaries.push(None);
                        for entry in open.iter_mut().filter(|entry| entry.first.is_none()) {
                            entry.first = Some(position);
                        }
                    }
                }
                WalkEvent::Leave(php_syntax::SyntaxElement::Node(_)) => {
                    let Some(entry) = open.pop() else {
                        continue;
                    };
                    let Some(first) = entry.first else {
                        continue;
                    };
                    let semicolon_namespace = entry.node.kind() == NAMESPACE_DECLARATION
                        && entry.node.children().any(|child| child.kind() == STATEMENT_LIST);
                    if entry.continuation && !semicolon_namespace {
                        let last = sig.len() - 1;
                        let anchor = after_attributes(&entry.node, &leaves, &sig, first).min(last);
                        starts_raw.push((anchor, entry.depth, entry.node.clone()));
                        ends[last] += 1;
                    }
                    if let Some(parent) = entry.node.parent() {
                        let items = items_of(&parent);
                        if let Some(position) = items.iter().position(|item| *item == entry.node) {
                            let after_namespace = parent.kind() == STATEMENT_LIST
                                && parent.parent().is_some_and(|node| node.kind() == NAMESPACE_DECLARATION);
                            if position == 0 && after_namespace {
                                boundaries[first] = Some(Boundary {
                                    kind: entry.node.kind(),
                                    previous: NAMESPACE_DECLARATION,
                                });
                            }
                            if position > 0 {
                                boundaries[first] = Some(Boundary {
                                    kind: entry.node.kind(),
                                    previous: items[position - 1].kind(),
                                });
                            }
                        }
                    }
                }
                WalkEvent::Leave(_) => {}
            }
        }
        // A close tag ends the file, or the file has markup after it.
        for (position, &leaf) in sig.iter().enumerate() {
            if leaves[leaf].kind == CLOSE_TAG && position + 1 != sig.len() {
                return Err(Refusal::Markup);
            }
        }
        starts_raw.sort_by_key(|(first, depth, _)| (*first, *depth));
        let mut starts: Vec<Vec<SyntaxNode>> = vec![Vec::new(); sig.len()];
        for (first, _, node) in starts_raw {
            starts[first].push(node);
        }
        Ok(Model {
            text,
            leaves,
            sig,
            starts,
            ends,
            boundaries,
        })
    }

    pub(crate) fn leaf(&self, position: usize) -> &Leaf {
        &self.leaves[self.sig[position]]
    }

    /// The kind of a token, where a keyword used as a name is a plain identifier.
    pub(crate) fn kind(&self, position: usize) -> SyntaxKind {
        let leaf = self.leaf(position);
        if leaf.kind.is_keyword() && leaf.parent.kind() == NAME {
            IDENT
        } else {
            leaf.kind
        }
    }

    pub(crate) fn parent(&self, position: usize) -> SyntaxKind {
        self.leaf(position).parent.kind()
    }

    pub(crate) fn grandparent(&self, position: usize) -> SyntaxKind {
        self.leaf(position)
            .parent
            .parent()
            .map_or(SOURCE_FILE, |node| node.kind())
    }
}
