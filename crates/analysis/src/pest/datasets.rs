//! Datasets: the names `->with('name')` and `dataset('name', ...)` share, and the types a dataset
//! gives the parameters of a test closure.

use php_index::test_facts::{argument_expressions, flatten_chain, read_statement, string_value};
use php_index::{Index, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::calls::{Dsl, dsl_of, top_statements};
use crate::ast::{child_of, range_of};
use crate::decl::Declaration;
use crate::infer::Analyzer;
use crate::phpunit::complete::Candidate;
use crate::phpunit::strings::{StringTarget, TestString};

/// The strings of a Pest call that name a dataset: the name `dataset()` declares and the names
/// `->with()` uses on a test.
pub fn call_strings(call: &SyntaxNode) -> Vec<TestString> {
    let Some(chain) = flatten_chain(call) else {
        return Vec::new();
    };
    let Some(root) = dsl_of(&chain[0].name) else {
        return Vec::new();
    };
    let last = chain.len() - 1;
    let (arguments, declaration) = match (last, root) {
        (0, Dsl::Dataset) => (chain[0].arguments.iter().take(1).collect::<Vec<_>>(), true),
        (_, Dsl::Test | Dsl::It | Dsl::Arch) if chain[last].name.eq_ignore_ascii_case("with") => {
            (chain[last].arguments.iter().collect(), false)
        }
        _ => return Vec::new(),
    };
    arguments
        .into_iter()
        .filter_map(string_value)
        .map(|(value, span)| TestString {
            range: range_of(span.start, span.end),
            value,
            target: StringTarget::Dataset,
            declaration,
        })
        .collect()
}

/// Where datasets of this name are declared in the project.
pub fn dataset_declarations(index: &Index, name: &str) -> Vec<Declaration> {
    index
        .test_files()
        .flat_map(|(file, facts)| {
            facts
                .datasets
                .iter()
                .filter(|dataset| dataset.name == name)
                .map(|dataset| Declaration {
                    path: file.path.clone(),
                    origin: file.origin,
                    name_span: dataset.span,
                    span: dataset.span,
                })
        })
        .collect()
}

/// The datasets a `->with('')` can name: the ones the project declares and the ones of the file.
pub fn dataset_candidates(index: &Index, root: &SyntaxNode) -> Vec<Candidate> {
    let mut names: Vec<(String, Option<String>)> = index
        .test_files()
        .flat_map(|(file, facts)| {
            let origin = file.path.file_name().map(|name| name.to_string_lossy().into_owned());
            facts
                .datasets
                .iter()
                .map(move |dataset| (dataset.name.clone(), origin.clone()))
        })
        .collect();
    let mut own = php_index::test_facts::TestFacts::default();
    let resolver = php_index::extract::resolver_at(root, u32::MAX);
    for statement in top_statements(root) {
        read_statement(&statement, &resolver, &mut own);
    }
    names.extend(own.datasets.into_iter().map(|dataset| (dataset.name, None)));
    names.sort();
    names.dedup_by(|later, earlier| later.0 == earlier.0);
    names
        .into_iter()
        .map(|(name, file)| Candidate {
            name,
            kind: crate::completion::ItemKind::Constant,
            detail: None,
            description: file,
            rank: 0,
        })
        .collect()
}

/// The types a dataset gives the parameters of a test closure, by position, when the closure is
/// the test of an `it()` or `test()` that has exactly one `->with()` and the rows are written out.
pub fn dataset_hints(analyzer: &Analyzer<'_>, closure: &SyntaxNode) -> Option<Vec<Type>> {
    let call = closure
        .parent()
        .filter(|parent| parent.kind() == ARGUMENT)?
        .parent()
        .and_then(|list| list.parent())
        .filter(|call| call.kind() == CALL_EXPR)?;
    let callee = call.children().next()?;
    if callee.kind() != NAME || !matches!(dsl_of(&callee.text().to_string()), Some(Dsl::Test | Dsl::It)) {
        return None;
    }
    let source = with_source(&call)?;
    let rows = match source.kind() {
        LITERAL => {
            let (name, _) = string_value(&source)?;
            return named_dataset_hints(analyzer, &name);
        }
        _ => rows_of(&source)?,
    };
    row_types(analyzer, &rows)
}

/// What the only `->with()` after a test call is given.
fn with_source(test: &SyntaxNode) -> Option<SyntaxNode> {
    let mut withs = Vec::new();
    let mut current = test.clone();
    while let Some(fetch) = current.parent().filter(|parent| parent.kind() == PROPERTY_FETCH_EXPR) {
        let outer = fetch.parent().filter(|parent| parent.kind() == CALL_EXPR)?;
        if child_of(&fetch, NAME).is_some_and(|name| name.text().to_string().eq_ignore_ascii_case("with")) {
            withs.push(outer.clone());
        }
        current = outer;
    }
    let [only] = withs.as_slice() else {
        return None;
    };
    let arguments = argument_expressions(only);
    let [source] = arguments.as_slice() else {
        return None;
    };
    Some(source.clone())
}

/// The datasets `dataset('name', ...)` declares in this file and in the others of the project.
fn named_dataset_hints(analyzer: &Analyzer<'_>, name: &str) -> Option<Vec<Type>> {
    let value = declared_value(analyzer, name)?;
    let rows = rows_of(&value.0)?;
    row_types(&value.1, &rows)
}

/// The value of a `dataset('name', value)` call and an analyzer for the tree it is in.
fn declared_value<'a>(analyzer: &Analyzer<'a>, name: &str) -> Option<(SyntaxNode, Analyzer<'a>)> {
    let find = |root: &SyntaxNode| -> Option<SyntaxNode> {
        for statement in top_statements(root) {
            let expression = statement.children().next()?;
            let chain = flatten_chain(&expression)?;
            if chain.len() == 1
                && dsl_of(&chain[0].name) == Some(Dsl::Dataset)
                && chain[0]
                    .arguments
                    .first()
                    .and_then(string_value)
                    .is_some_and(|(found, _)| found == name)
            {
                return chain[0].arguments.get(1).cloned();
            }
        }
        None
    };
    if let Some(value) = find(&analyzer.root) {
        let offset = crate::ast::start(&value);
        let own = Analyzer::with_shared(analyzer.index, &analyzer.root, offset, analyzer.shared.clone());
        return Some((value, own));
    }
    let declaration = dataset_declarations(analyzer.index, name).into_iter().next()?;
    let root = analyzer.parse_file(&declaration.path)?;
    let value = find(&root)?;
    let offset = crate::ast::start(&value);
    let foreign = Analyzer::with_shared(analyzer.index, &root, offset, analyzer.shared.clone());
    Some((value, foreign))
}

/// The rows of a dataset written out: an array, or a closure that returns one.
fn rows_of(source: &SyntaxNode) -> Option<Vec<SyntaxNode>> {
    let table = match source.kind() {
        ARRAY_EXPR => source.clone(),
        CLOSURE_EXPR | ARROW_FUNCTION_EXPR => {
            let value = if source.kind() == ARROW_FUNCTION_EXPR {
                source.children().last()?
            } else {
                let body = child_of(source, BLOCK)?;
                let returns: Vec<SyntaxNode> = body
                    .descendants()
                    .filter(|node| node.kind() == RETURN_STATEMENT)
                    .collect();
                let [only] = returns.as_slice() else {
                    return None;
                };
                only.children().next()?
            };
            if value.kind() != ARRAY_EXPR {
                return None;
            }
            value
        }
        _ => return None,
    };
    let mut rows = Vec::new();
    for item in table.children().filter(|child| child.kind() == ARRAY_ITEM) {
        if item.children_with_tokens().any(|element| element.kind() == ELLIPSIS) {
            return None;
        }
        rows.push(item.children().last()?);
    }
    (!rows.is_empty()).then_some(rows)
}

/// The type each position of the rows has: all rows are arrays and give the position the same kind
/// of value, or all rows are single values for the first parameter.
fn row_types(analyzer: &Analyzer<'_>, rows: &[SyntaxNode]) -> Option<Vec<Type>> {
    let env = crate::infer::Env::default();
    if rows.iter().all(|row| row.kind() == ARRAY_EXPR) {
        let mut columns: Vec<Vec<Type>> = Vec::new();
        for row in rows {
            let items: Vec<SyntaxNode> = row.children().filter(|child| child.kind() == ARRAY_ITEM).collect();
            for (position, item) in items.iter().enumerate() {
                let operands: Vec<SyntaxNode> = item.children().collect();
                let [value] = operands.as_slice() else {
                    return None;
                };
                if columns.len() <= position {
                    columns.resize(position + 1, Vec::new());
                }
                columns[position].push(analyzer.type_of(value, &env));
            }
        }
        return Some(
            columns
                .into_iter()
                .map(|types| {
                    if types.len() != rows.len() || types.iter().any(Type::is_unknown) {
                        Type::Unknown
                    } else {
                        Type::union(types)
                    }
                })
                .collect(),
        );
    }
    if rows.iter().any(|row| row.kind() == ARRAY_EXPR) {
        return None;
    }
    let types: Vec<Type> = rows.iter().map(|row| analyzer.type_of(row, &env)).collect();
    if types.iter().any(Type::is_unknown) {
        return None;
    }
    Some(vec![Type::union(types)])
}
