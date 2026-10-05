//! Call hierarchy and type hierarchy: who calls a function or method and what it calls, and the
//! supertypes and subtypes of a class.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use php_index::{Class, ClassKind, Index, Name, Span};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange, parse};

use crate::ast::{self, range_of};
use crate::context::FileContext;
use crate::references::{Sources, find_hits, symbols_at};
use crate::refs::{HitKind, Query, Symbol};

// Type hierarchy -------------------------------------------------------------------------------

/// A class, interface, trait or enum as a node of the hierarchy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeItem {
    pub name: Name,
    pub kind: ClassKind,
    pub path: PathBuf,
    pub span: Span,
    pub name_span: Span,
    pub deprecated: bool,
}

fn type_item(class: Class<'_>) -> TypeItem {
    TypeItem {
        name: class.decl.name.clone(),
        kind: class.decl.kind,
        path: class.file.path.clone(),
        span: class.decl.span,
        name_span: class.decl.name_span,
        deprecated: class.decl.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
    }
}

/// The class a position names or declares.
pub fn prepare_type_hierarchy(index: &Index, root: &php_syntax::SyntaxNode, offset: u32) -> Vec<TypeItem> {
    let Some((_, symbols)) = symbols_at(index, root, offset) else {
        return Vec::new();
    };
    symbols
        .iter()
        .filter_map(|symbol| match symbol {
            Symbol::Class(name) => index.class(name).map(type_item),
            _ => None,
        })
        .take(1)
        .collect()
}

/// What a class extends, implements and uses, in that order.
pub fn supertypes(index: &Index, name: &str) -> Vec<TypeItem> {
    let Some(class) = index.class(name) else {
        return Vec::new();
    };
    let decl = class.decl;
    let mut out: Vec<TypeItem> = Vec::new();
    let types = decl
        .extends
        .iter()
        .chain(&decl.implements)
        .chain(decl.trait_uses.iter().map(|usage| &usage.ty));
    for ty in types {
        for parent in ty.class_names() {
            if let Some(found) = index.class(parent) {
                let item = type_item(found);
                if !out
                    .iter()
                    .any(|existing| existing.name.eq_ignore_ascii_case(&item.name))
                {
                    out.push(item);
                }
            }
        }
    }
    out
}

/// The classes that extend, implement or use a class directly.
pub fn subtypes(index: &Index, name: &str) -> Vec<TypeItem> {
    let mut out: Vec<TypeItem> = index.direct_subtypes(name).into_iter().map(type_item).collect();
    out.sort_by(|left, right| left.name.cmp(&right.name));
    out
}

// Call hierarchy -------------------------------------------------------------------------------

/// What a call hierarchy item stands for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CallSymbol {
    Function(Name),
    Method {
        class: Name,
        name: String,
    },
    /// The code of a file that belongs to no function.
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallItem {
    pub symbol: CallSymbol,
    pub path: PathBuf,
    pub span: Span,
    pub name_span: Span,
    pub deprecated: bool,
}

/// The function or method a position names or declares.
pub fn prepare_call_hierarchy(index: &Index, root: &SyntaxNode, offset: u32) -> Vec<CallItem> {
    let Some((_, symbols)) = symbols_at(index, root, offset) else {
        return Vec::new();
    };
    symbols
        .iter()
        .find_map(|symbol| match symbol {
            Symbol::Function(name) => call_item(index, &CallSymbol::Function(name.clone())),
            Symbol::Method { class, name } => call_item(
                index,
                &CallSymbol::Method {
                    class: class.clone(),
                    name: name.clone(),
                },
            ),
            _ => None,
        })
        .into_iter()
        .collect()
}

/// The declaration of a function or method, as an item.
pub fn call_item(index: &Index, symbol: &CallSymbol) -> Option<CallItem> {
    match symbol {
        CallSymbol::Function(name) => {
            let function = index.function(name)?;
            Some(CallItem {
                symbol: symbol.clone(),
                path: function.file.path.clone(),
                span: function.decl.span,
                name_span: function.decl.name_span,
                deprecated: function.decl.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
            })
        }
        CallSymbol::Method { class, name } => {
            let class = index.class(class)?;
            let method = class.decl.method(name)?;
            Some(CallItem {
                symbol: symbol.clone(),
                path: class.file.path.clone(),
                span: method.span,
                name_span: method.name_span,
                deprecated: method.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
            })
        }
        CallSymbol::File(path) => Some(CallItem {
            symbol: symbol.clone(),
            path: path.clone(),
            span: Span::default(),
            name_span: Span::default(),
            deprecated: false,
        }),
    }
}

/// A caller with the places in it that make the call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncomingCall {
    pub from: CallItem,
    /// The ranges of the called names, in the file of the caller.
    pub ranges: Vec<TextRange>,
}

/// Everything that calls a function or method, or a method it overrides or implements.
pub fn incoming_calls(index: &Index, sources: &dyn Sources, symbol: &CallSymbol) -> Vec<IncomingCall> {
    let query = match symbol {
        CallSymbol::Function(name) => Query::new(index, Symbol::Function(name.clone())),
        CallSymbol::Method { class, name } => Query::new(
            index,
            Symbol::Method {
                class: class.clone(),
                name: name.clone(),
            },
        ),
        CallSymbol::File(_) => return Vec::new(),
    };
    let mut grouped: BTreeMap<(PathBuf, u32), IncomingCall> = BTreeMap::new();
    for file in find_hits(index, sources, None, &query) {
        let Some(text) = sources.text(&file.path) else {
            continue;
        };
        let root = parse(&text).syntax();
        let ctx = FileContext::new(index, &root);
        for hit in &file.hits {
            if hit.kind != HitKind::Reference {
                continue;
            }
            let Some(item) = enclosing_callable(&ctx, &file.path, u32::from(hit.range.start())) else {
                continue;
            };
            grouped
                .entry((file.path.clone(), item.span.start))
                .or_insert_with(|| IncomingCall {
                    from: item,
                    ranges: Vec::new(),
                })
                .ranges
                .push(hit.range);
        }
    }
    grouped.into_values().collect()
}

/// The named function or method a position is in, or the file when it is in none.
fn enclosing_callable(ctx: &FileContext, path: &Path, offset: u32) -> Option<CallItem> {
    let node = ast::node_at(&ctx.root, offset + 1);
    let function = node
        .ancestors()
        .find(|ancestor| matches!(ancestor.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION));
    let Some(function) = function else {
        return Some(CallItem {
            symbol: CallSymbol::File(path.to_path_buf()),
            path: path.to_path_buf(),
            span: Span {
                start: 0,
                end: ast::end(&ctx.root),
            },
            name_span: Span::default(),
            deprecated: false,
        });
    };
    let name = ast::child_of(&function, NAME)?;
    let analyzer = ctx.analyzer(&function);
    let symbol = match function.kind() {
        FUNCTION_DECLARATION => CallSymbol::Function(analyzer.resolver.qualify(&ast::text_of(&name))),
        _ => CallSymbol::Method {
            class: analyzer.class.as_ref()?.name.clone(),
            name: ast::text_of(&name),
        },
    };
    Some(CallItem {
        symbol,
        path: path.to_path_buf(),
        span: Span {
            start: ast::start(&function),
            end: ast::end(&function),
        },
        name_span: Span {
            start: ast::start(&name),
            end: ast::end(&name),
        },
        deprecated: false,
    })
}

/// A callee with the places in the caller that call it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingCall {
    pub to: CallItem,
    pub ranges: Vec<TextRange>,
}

/// Everything a function or method calls in its body, by callee.
pub fn outgoing_calls(index: &Index, text: &str, item: &CallItem) -> Vec<OutgoingCall> {
    if matches!(item.symbol, CallSymbol::File(_)) {
        return Vec::new();
    }
    let root = parse(text).syntax();
    let range = range_of(item.span.start, item.span.end);
    let function = match root.covering_element(range) {
        php_syntax::SyntaxElement::Node(node) => node,
        php_syntax::SyntaxElement::Token(token) => match token.parent() {
            Some(parent) => parent,
            None => return Vec::new(),
        },
    };
    let Some(function) = function
        .ancestors()
        .find(|node| matches!(node.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION))
    else {
        return Vec::new();
    };
    let ctx = FileContext::new(index, &root);
    let mut grouped: Vec<(CallSymbol, Vec<TextRange>)> = Vec::new();
    for node in function.descendants() {
        if !matches!(node.kind(), CALL_EXPR | NEW_EXPR) || in_nested_declaration(&node, &function) {
            continue;
        }
        let analyzer = ctx.analyzer(&node);
        let env = analyzer.env_around(&node);
        let Some(called_range) = called_name_range(&node) else {
            continue;
        };
        for callee in analyzer.callees(&node, &env) {
            let symbol = match callee.name.rsplit_once("::") {
                Some((class, method)) => CallSymbol::Method {
                    class: class.to_string(),
                    name: method.to_string(),
                },
                None if callee.name == "closure" => continue,
                None => CallSymbol::Function(callee.name.clone()),
            };
            match grouped.iter_mut().find(|(existing, _)| *existing == symbol) {
                Some((_, ranges)) => ranges.push(called_range),
                None => grouped.push((symbol, vec![called_range])),
            }
        }
    }
    grouped
        .into_iter()
        .filter_map(|(symbol, ranges)| {
            Some(OutgoingCall {
                to: call_item(index, &symbol)?,
                ranges,
            })
        })
        .collect()
}

fn in_nested_declaration(node: &SyntaxNode, function: &SyntaxNode) -> bool {
    node.ancestors()
        .take_while(|ancestor| ancestor != function)
        .any(|ancestor| {
            matches!(ancestor.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION) || ast::is_class_like(ancestor.kind())
        })
}

/// The name a call goes by: the function, the member after `->` or `::`, or the class of a `new`.
fn called_name_range(call: &SyntaxNode) -> Option<TextRange> {
    let name = match call.kind() {
        NEW_EXPR => call.children().find(|child| child.kind() == NAME),
        _ => {
            let callee = call.children().next()?;
            match callee.kind() {
                NAME => Some(callee),
                PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR => {
                    callee.children().filter(|child| child.kind() == NAME).last()
                }
                _ => None,
            }
        }
    }?;
    Some(
        name.last_token()
            .map_or_else(|| name.text_range(), |token| ast::last_segment_of_token(&token)),
    )
}
