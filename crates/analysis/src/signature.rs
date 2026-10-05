//! Signature help: the parameters of the function a cursor is calling, the one under the cursor,
//! and every way the function can be called.

use php_index::{Callable, Doc, Index};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use crate::ast::{self, child_of, end, text_of};
use crate::infer::Analyzer;
use crate::infer::arguments;
use crate::render;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterItem {
    pub label: String,
    pub documentation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureItem {
    pub label: String,
    pub documentation: Option<String>,
    pub parameters: Vec<ParameterItem>,
    /// The parameter the cursor is in, for this way of calling the function. It lies past the last
    /// parameter when there are too many arguments.
    pub active_parameter: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureHelp {
    pub signatures: Vec<SignatureItem>,
    pub active_signature: usize,
}

struct Candidate {
    name: String,
    callable: Callable,
    doc: Option<Doc>,
}

/// The signatures of the call around the offset.
pub fn signature_help(index: &Index, root: &SyntaxNode, offset: u32) -> Option<SignatureHelp> {
    let list = argument_list_at(root, offset)?;
    let call = list.parent()?;
    let analyzer = Analyzer::new(index, root, offset);
    let candidates = match call.kind() {
        ATTRIBUTE => attribute_candidates(&analyzer, &call),
        CALL_EXPR | NEW_EXPR => call_candidates(&analyzer, &call),
        _ => Vec::new(),
    };
    if candidates.is_empty() {
        return None;
    }
    let level = analyzer.level();
    let args = arguments(&call);
    let position = argument_position(&list, offset);
    let named = args.get(position).and_then(|arg| arg.name.clone());
    let named_arguments: Vec<&str> = args.iter().filter_map(|arg| arg.name.as_deref()).collect();
    let needed = args.len().max(position + 1);
    let mut signatures: Vec<SignatureItem> = Vec::new();
    let mut active_signature = None;
    for candidate in &candidates {
        let params: Vec<&php_index::Param> = candidate.callable.params_at(level).collect();
        let labels: Vec<String> = params.iter().map(|param| render::param_text(param, level)).collect();
        let mut label = format!("{}({})", candidate.name, labels.join(", "));
        let ret = candidate
            .callable
            .native_return(level)
            .or_else(|| candidate.callable.effective_return(level));
        if let Some(ret) = ret {
            label.push_str(&format!(": {}", ret.display(true)));
        }
        if signatures.iter().any(|existing| existing.label == label) {
            continue;
        }
        let variadic = params.last().is_some_and(|param| param.variadic);
        let fits = (params.len() >= needed || variadic)
            && named_arguments
                .iter()
                .all(|name| params.iter().any(|param| param.name == *name));
        if fits && active_signature.is_none() {
            active_signature = Some(signatures.len());
        }
        let active = match &named {
            Some(name) => params.iter().position(|param| &param.name == name),
            None if position < params.len() || !variadic => Some(position),
            None => Some(params.len() - 1),
        };
        signatures.push(SignatureItem {
            label,
            documentation: candidate.doc.as_ref().and_then(summary),
            parameters: params
                .iter()
                .zip(labels)
                .map(|(param, label)| ParameterItem {
                    label,
                    documentation: (!param.description.is_empty()).then(|| param.description.clone()),
                })
                .collect(),
            active_parameter: active,
        });
    }
    Some(SignatureHelp {
        signatures,
        active_signature: active_signature.unwrap_or(0),
    })
}

fn summary(doc: &Doc) -> Option<String> {
    let mut out = doc.summary.clone();
    if !doc.description.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&doc.description);
    }
    (!out.is_empty()).then_some(out)
}

/// The innermost argument list that the offset is inside of: after its `(` and before its `)`, or
/// at the end of one that was never closed.
fn argument_list_at(root: &SyntaxNode, offset: u32) -> Option<SyntaxNode> {
    let mut anchor = root.token_at_offset(php_syntax::TextSize::from(offset)).left_biased();
    while let Some(token) = anchor.clone().filter(|token| token.kind().is_trivia()) {
        anchor = token.prev_token();
    }
    anchor?.parent_ancestors().find(|node| {
        if node.kind() != ARGUMENT_LIST {
            return false;
        }
        let Some(open) = ast::first_token(node, LPAREN) else {
            return false;
        };
        let after_open = u32::from(open.text_range().end()) <= offset;
        let before_close = match ast::first_token(node, RPAREN) {
            Some(close) => offset <= u32::from(close.text_range().start()),
            None => offset <= end(node) || only_trivia_between(node, offset),
        };
        after_open && before_close
    })
}

/// Whether nothing but whitespace and comments lies between the end of a node and an offset.
fn only_trivia_between(node: &SyntaxNode, offset: u32) -> bool {
    let mut next = node.last_token().and_then(|token| token.next_token());
    while let Some(token) = next {
        if u32::from(token.text_range().start()) >= offset {
            return true;
        }
        if !token.kind().is_trivia() {
            return false;
        }
        next = token.next_token();
    }
    true
}

/// Which argument the offset is in: the number of commas of the list before it.
fn argument_position(list: &SyntaxNode, offset: u32) -> usize {
    ast::tokens(list)
        .filter(|token| token.kind() == COMMA && u32::from(token.text_range().end()) <= offset)
        .count()
}

fn call_candidates(analyzer: &Analyzer<'_>, call: &SyntaxNode) -> Vec<Candidate> {
    let env = analyzer.env_around(call);
    let mut out = Vec::new();
    for callee in analyzer.callees(call, &env) {
        match callee.name.rsplit_once("::") {
            Some((class, method)) => {
                let methods = analyzer
                    .index
                    .class(class)
                    .map(|found| {
                        found
                            .decl
                            .methods
                            .iter()
                            .filter(|candidate| {
                                candidate.name.eq_ignore_ascii_case(method)
                                    && candidate.availability.contains(analyzer.level())
                            })
                            .map(|candidate| Candidate {
                                name: candidate.name.clone(),
                                callable: candidate.callable.clone(),
                                doc: candidate.doc.as_deref().cloned(),
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if methods.is_empty() {
                    out.push(Candidate {
                        name: method.to_string(),
                        callable: callee.callable.clone(),
                        doc: callee.doc.as_deref().cloned(),
                    });
                } else {
                    out.extend(methods);
                }
            }
            None => {
                let overloads = analyzer.index.function_overloads(&callee.name);
                if overloads.is_empty() {
                    out.push(Candidate {
                        name: crate::short(&callee.name).to_string(),
                        callable: callee.callable.clone(),
                        doc: callee.doc.as_deref().cloned(),
                    });
                } else {
                    out.extend(overloads.into_iter().map(|function| Candidate {
                        name: crate::short(&function.decl.name).to_string(),
                        callable: function.decl.callable.clone(),
                        doc: function.decl.doc.as_deref().cloned(),
                    }));
                }
            }
        }
    }
    out
}

fn attribute_candidates(analyzer: &Analyzer<'_>, attribute: &SyntaxNode) -> Vec<Candidate> {
    let Some(name) = child_of(attribute, NAME) else {
        return Vec::new();
    };
    let class = analyzer.class_type(&text_of(&name));
    analyzer
        .index
        .find_method(&class, "__construct")
        .map(|found| Candidate {
            name: "__construct".to_string(),
            callable: found.member.callable.clone(),
            doc: found.member.doc.as_deref().cloned(),
        })
        .into_iter()
        .collect()
}
