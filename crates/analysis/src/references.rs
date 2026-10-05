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
    let token = token_at(root, offset)?;
    let symbols = symbols_of_token(&ctx, &token);
    if symbols.is_empty() {
        return None;
    }
    Some((ast::last_segment_of_token(&token), symbols))
}

/// Every place that names what is under a position.
pub fn references_at(index: &Index, sources: &dyn Sources, current: &Current, offset: u32) -> Option<References> {
    let (range, symbols) = symbols_at(index, current.root, offset)?;
    let files = hits_of_symbols(index, sources, current, &symbols);
    Some(References { range, symbols, files })
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
        let ctx = FileContext::new(index, current.root);
        return vec![FileHits {
            path: current.path.to_path_buf(),
            hits: variable_hits(&ctx, *scope, name),
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
            let root = parse(&text).syntax();
            let ctx = FileContext::new(index, &root);
            let hits = hits_in_file(&ctx, &text, query);
            (!hits.is_empty()).then(|| FileHits {
                path: path.clone(),
                hits,
            })
        })
        .collect();
    if let Some(current) = current {
        let ctx = FileContext::new(index, current.root);
        let own = hits_in_file(&ctx, current.text, query);
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
    let Some((_, symbols)) = symbols_at(index, current.root, offset) else {
        return Vec::new();
    };
    let ctx = FileContext::new(index, current.root);
    let mut hits: Vec<Hit> = Vec::new();
    for symbol in &symbols {
        match symbol {
            Symbol::Variable { name, scope } => hits.extend(variable_hits(&ctx, *scope, name)),
            other => {
                let query = Query::new(index, other.clone());
                hits.extend(hits_in_file(&ctx, current.text, &query));
            }
        }
    }
    hits.sort_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits.dedup_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits
}
