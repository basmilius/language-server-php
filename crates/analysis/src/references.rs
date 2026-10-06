//! Find usages: the symbol under a position and every place in the project that names it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use php_index::Index;
use php_syntax::{SyntaxNode, TextRange, parse};
use rayon::prelude::*;

use crate::ast::{self, range_of};
use crate::context::FileContext;
use crate::decl::declarations;
use crate::refs::{Hit, Query, Symbol, hits_in_file, symbols_of_token, variable_hits};
use crate::target::token_at;

/// The files a search reads. A front end answers with what it keeps: open documents first, then
/// the files of the project.
pub trait Sources: Sync {
    /// The files that may mention a word, given in lowercase.
    fn candidates(&self, word: &str) -> Vec<PathBuf>;

    /// The current text of a file.
    fn text(&self, path: &Path) -> Option<String>;
}

/// The file a question is asked in, as the front end holds it.
pub struct Current<'a> {
    pub path: &'a Path,
    pub text: &'a str,
    pub root: &'a SyntaxNode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileHits {
    pub path: PathBuf,
    pub hits: Vec<Hit>,
}

#[derive(Clone, Debug)]
pub struct References {
    /// The name under the position.
    pub range: TextRange,
    /// What the name stands for. A promoted constructor parameter stands for a variable, a
    /// parameter and a property at once.
    pub symbols: Vec<Symbol>,
    pub files: Vec<FileHits>,
}

/// The symbols under a position, with the range of the name they were found on.
pub fn symbols_at(index: &Index, root: &SyntaxNode, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    let ctx = FileContext::new(index, root);
    if let Some(found) = crate::phpunit::strings::symbols_at_string(&ctx, offset) {
        return Some(found);
    }
    if let Some(found) = key_symbol_at(index, root, offset) {
        return Some(found);
    }
    let token = token_at(root, offset)?;
    let symbols = symbols_of_token(&ctx, &token);
    if symbols.is_empty() {
        return None;
    }
    Some((ast::last_segment_of_token(&token), symbols))
}

/// A string that names a route, a config key or the like.
fn key_symbol_at(index: &Index, root: &SyntaxNode, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    if !index.frameworks().any() {
        return None;
    }
    let key = crate::frameworks::keys::key_at(&crate::infer::Analyzer::new(index, root, offset), offset)?;
    Some((
        key.range,
        vec![Symbol::Key {
            kind: key.kind,
            name: key.value,
            scope: key.scope,
        }],
    ))
}

/// The name a framework file declares at a position: the `->name('home')` of a route, a key of a
/// config or translation file. Only a string is looked at.
fn declared_key_at(index: &Index, current: &Current, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    if !index.frameworks().any() {
        return None;
    }
    use php_syntax::SyntaxKind::{ATTRIBUTE, CLASS_DECLARATION, METHOD_DECLARATION, NAME, STRING_LITERAL};
    let string = match current.root.token_at_offset(php_syntax::TextSize::from(offset)) {
        php_syntax::TokenAtOffset::None => None,
        php_syntax::TokenAtOffset::Single(token) => Some(token),
        php_syntax::TokenAtOffset::Between(left, right) => {
            [right, left].into_iter().find(|token| token.kind() == STRING_LITERAL)
        }
    }
    .filter(|token| token.kind() == STRING_LITERAL)?;
    let key = |kind, name| {
        vec![Symbol::Key {
            kind,
            name,
            scope: None,
        }]
    };
    if let Some((kind, name, span)) = php_index::framework::keys::declared_at(index, current.path, offset) {
        return Some((range_of(span.start, span.end), key(kind, name)));
    }
    let literal = string.parent()?;
    let (value, span) = php_index::test_facts::string_value(&literal)?;
    let attribute = literal.ancestors().find(|node| node.kind() == ATTRIBUTE)?;
    let owner = attribute
        .ancestors()
        .find(|node| matches!(node.kind(), METHOD_DECLARATION | CLASS_DECLARATION))?;
    let name = owner.children().find(|child| child.kind() == NAME)?;
    let owner_span = php_index::Span {
        start: u32::from(name.text_range().start()),
        end: u32::from(name.text_range().end()),
    };
    let (kind, name) = php_index::framework::keys::declared_by(index, current.path, owner_span, &value)?;
    Some((range_of(span.start, span.end), key(kind, name)))
}

/// The symbols under a position of the file a question is asked in, a Blade template included.
pub fn symbols_in_current(index: &Index, current: &Current, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    if crate::blade::is_template(current.path) {
        return crate::blade::symbols_at(index, Some(current.path), current.text, offset);
    }
    symbols_at(index, current.root, offset).or_else(|| declared_key_at(index, current, offset))
}

/// Every place that names what is under a position.
pub fn references_at(index: &Index, sources: &dyn Sources, current: &Current, offset: u32) -> Option<References> {
    let (range, symbols) = symbols_in_current(index, current, offset)?;
    let files = hits_of_symbols(index, sources, current, &symbols);
    Some(References { range, symbols, files })
}

/// The places of a query in one file, which is a Blade template or PHP.
fn hits_in_text(index: &Index, path: &Path, text: &str, root: Option<&SyntaxNode>, query: &Query) -> Vec<Hit> {
    if crate::blade::is_template(path) {
        return crate::blade::hits(index, Some(path), text, query);
    }
    let parsed;
    let root = match root {
        Some(root) => root,
        None => {
            parsed = parse(text).syntax();
            &parsed
        }
    };
    hits_in_file(&FileContext::new(index, root), text, query)
}

/// The places that name any of the symbols, by file.
pub fn hits_of_symbols(index: &Index, sources: &dyn Sources, current: &Current, symbols: &[Symbol]) -> Vec<FileHits> {
    let mut merged: BTreeMap<PathBuf, Vec<Hit>> = BTreeMap::new();
    for symbol in symbols {
        for found in hits_of_symbol(index, sources, current, symbol) {
            merged.entry(found.path).or_default().extend(found.hits);
        }
    }
    merged
        .into_iter()
        .map(|(path, mut hits)| {
            hits.sort_by_key(|hit| (hit.range.start(), hit.range.end()));
            hits.dedup_by_key(|hit| (hit.range.start(), hit.range.end()));
            FileHits { path, hits }
        })
        .filter(|found| !found.hits.is_empty())
        .collect()
}

fn hits_of_symbol(index: &Index, sources: &dyn Sources, current: &Current, symbol: &Symbol) -> Vec<FileHits> {
    if let Symbol::Variable { name, scope } = symbol {
        let hits = if crate::blade::is_template(current.path) {
            let query = Query::new(index, symbol.clone());
            crate::blade::hits(index, Some(current.path), current.text, &query)
        } else {
            variable_hits(&FileContext::new(index, current.root), *scope, name)
        };
        return vec![FileHits {
            path: current.path.to_path_buf(),
            hits,
        }];
    }
    let query = Query::new(index, symbol.clone());
    let mut out = find_hits(index, sources, Some(current), &query);
    if matches!(symbol, Symbol::Parameter { .. }) {
        out.extend(parameter_body_hits(index, sources, current, &query));
    }
    out
}

/// The places of a query in the current file, if there is one, and in the files the sources think
/// may hold it.
pub fn find_hits(index: &Index, sources: &dyn Sources, current: Option<&Current>, query: &Query) -> Vec<FileHits> {
    let mut paths: Vec<PathBuf> = query
        .words()
        .iter()
        .flat_map(|word| sources.candidates(word))
        .filter(|path| current.is_none_or(|current| path != current.path))
        .collect();
    paths.sort();
    paths.dedup();
    let mut out: Vec<FileHits> = paths
        .par_iter()
        .filter_map(|path| {
            let text = sources.text(path)?;
            let hits = hits_in_text(index, path, &text, None, query);
            (!hits.is_empty()).then(|| FileHits {
                path: path.clone(),
                hits,
            })
        })
        .collect();
    if let Some(current) = current {
        let own = hits_in_text(index, current.path, current.text, Some(current.root), query);
        if !own.is_empty() {
            out.push(FileHits {
                path: current.path.to_path_buf(),
                hits: own,
            });
        }
    }
    out
}

/// A parameter is also a variable inside the function that declares it, and a `@param` of its doc.
fn parameter_body_hits(index: &Index, sources: &dyn Sources, current: &Current, query: &Query) -> Vec<FileHits> {
    let Symbol::Parameter { name, .. } = &query.symbol else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for declaration in declarations(index, query) {
        if crate::blade::is_template(&declaration.path) {
            continue;
        }
        let owned;
        let (text, root): (&str, SyntaxNode) = if declaration.path == current.path {
            (current.text, current.root.clone())
        } else {
            let Some(text) = sources.text(&declaration.path) else {
                continue;
            };
            owned = text;
            (owned.as_str(), parse(&owned).syntax())
        };
        let _ = text;
        let span = range_of(declaration.span.start, declaration.span.end);
        let function = match root.covering_element(span) {
            php_syntax::SyntaxElement::Node(node) => node,
            php_syntax::SyntaxElement::Token(token) => match token.parent() {
                Some(parent) => parent,
                None => continue,
            },
        };
        let Some(function) = ast::enclosing_function(&function) else {
            continue;
        };
        let ctx = FileContext::new(index, &root);
        let hits = variable_hits(&ctx, (ast::start(&function), ast::end(&function)), name);
        if !hits.is_empty() {
            out.push(FileHits {
                path: declaration.path.clone(),
                hits,
            });
        }
    }
    out
}

/// The places of the current file that name what is under a position.
pub fn highlights_at(index: &Index, current: &Current, offset: u32) -> Vec<Hit> {
    let Some((_, symbols)) = symbols_in_current(index, current, offset) else {
        return Vec::new();
    };
    let ctx = FileContext::new(index, current.root);
    let mut hits: Vec<Hit> = Vec::new();
    for symbol in &symbols {
        match symbol {
            Symbol::Variable { name, scope } if !crate::blade::is_template(current.path) => {
                hits.extend(variable_hits(&ctx, *scope, name))
            }
            other => {
                let query = Query::new(index, other.clone());
                hits.extend(hits_in_text(
                    index,
                    current.path,
                    current.text,
                    Some(current.root),
                    &query,
                ));
            }
        }
    }
    hits.sort_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits.dedup_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits
}
