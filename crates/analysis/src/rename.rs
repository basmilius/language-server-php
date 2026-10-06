//! Rename: which names can change, what a new name has to satisfy, and the edits that follow from
//! the places a symbol is used. Strings and comments are left alone, and so is everything outside
//! the project's own files.

use std::collections::BTreeMap;
use std::path::PathBuf;

use php_index::{Index, Origin};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange, parse};

use crate::ast::{self, range_of};
use crate::context::FileContext;
use crate::decl::declarations;
use crate::references::{Current, Sources, hits_of_symbols, symbols_at};
use crate::refs::{Query, Symbol, declares_method, declares_property};
use crate::target::token_at;

const RESERVED: &[&str] = &[
    "abstract",
    "and",
    "array",
    "as",
    "break",
    "callable",
    "case",
    "catch",
    "class",
    "clone",
    "const",
    "continue",
    "declare",
    "default",
    "die",
    "do",
    "echo",
    "else",
    "elseif",
    "empty",
    "enddeclare",
    "endfor",
    "endforeach",
    "endif",
    "endswitch",
    "endwhile",
    "eval",
    "exit",
    "extends",
    "final",
    "finally",
    "fn",
    "for",
    "foreach",
    "function",
    "global",
    "goto",
    "if",
    "implements",
    "include",
    "include_once",
    "instanceof",
    "insteadof",
    "interface",
    "isset",
    "list",
    "match",
    "namespace",
    "new",
    "or",
    "print",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "require_once",
    "return",
    "static",
    "switch",
    "throw",
    "trait",
    "try",
    "unset",
    "use",
    "var",
    "while",
    "xor",
    "yield",
];

/// Names a class, interface, trait, enum, function or constant cannot have.
const RESERVED_TYPES: &[&str] = &[
    "self", "parent", "static", "int", "float", "bool", "string", "true", "false", "null", "void", "iterable",
    "object", "mixed", "never", "array", "callable", "resource", "numeric",
];

const SUPERGLOBALS: &[&str] = &[
    "GLOBALS", "_SERVER", "_GET", "_POST", "_FILES", "_COOKIE", "_SESSION", "_REQUEST", "_ENV", "this",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenameKind {
    Class,
    Function,
    Constant,
    Method,
    Property,
    ClassConst,
    Variable,
    Namespace,
    Dataset,
    Key,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prepared {
    /// The name as the editor should offer it for editing.
    pub range: TextRange,
    pub placeholder: String,
    pub kind: RenameKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: TextRange,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEdits {
    pub path: PathBuf,
    pub edits: Vec<TextEdit>,
}

/// The file of a renamed class, which carries the class's name. Whether its new place follows the
/// project's autoload rules is for the caller to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRename {
    pub from: PathBuf,
    pub to: PathBuf,
    /// The class as it was named before the rename.
    pub class: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rename {
    pub files: Vec<FileEdits>,
    pub file_rename: Option<FileRename>,
}

fn kind_of(symbol: &Symbol) -> RenameKind {
    match symbol {
        Symbol::Class(_) => RenameKind::Class,
        Symbol::Function(_) => RenameKind::Function,
        Symbol::Constant(_) => RenameKind::Constant,
        Symbol::Method { .. } => RenameKind::Method,
        Symbol::Property { .. } => RenameKind::Property,
        Symbol::ClassConst { .. } => RenameKind::ClassConst,
        Symbol::Parameter { .. } | Symbol::Variable { .. } => RenameKind::Variable,
        Symbol::Dataset(_) => RenameKind::Dataset,
        Symbol::Key { .. } => RenameKind::Key,
    }
}

/// The name under a position, when it can be renamed.
pub fn prepare_rename(index: &Index, root: &SyntaxNode, text: &str, offset: u32) -> Result<Prepared, String> {
    if let Some((range, symbols)) = crate::phpunit::strings::symbols_at_string(&FileContext::new(index, root), offset) {
        check_declared_in_project(index, &symbols)?;
        return Ok(Prepared {
            placeholder: text[usize::from(range.start())..usize::from(range.end())].to_string(),
            range,
            kind: kind_of(primary_symbol(&symbols)),
        });
    }
    let Some(token) = token_at(root, offset) else {
        return Err("There is no name to rename here".to_string());
    };
    if let Some(prepared) = prepare_namespace(&token) {
        return Ok(prepared);
    }
    if matches!(
        token.text().to_ascii_lowercase().as_str(),
        "self" | "static" | "parent" | "$this"
    ) {
        return Err(format!("'{}' is a keyword and cannot be renamed", token.text()));
    }
    let Some((range, symbols)) = symbols_at(index, root, offset) else {
        return Err("There is no name to rename here".to_string());
    };
    let primary = primary_symbol(&symbols);
    check_declared_in_project(index, &symbols)?;
    let range = if token.kind() == VARIABLE {
        range_of(u32::from(range.start()) + 1, u32::from(range.end()))
    } else {
        range
    };
    Ok(Prepared {
        placeholder: text[usize::from(range.start())..usize::from(range.end())].to_string(),
        range,
        kind: kind_of(primary),
    })
}

fn prepare_namespace(token: &php_syntax::SyntaxToken) -> Option<Prepared> {
    let name = token.parent().filter(|parent| parent.kind() == NAME)?;
    let declaration = name.parent().filter(|parent| parent.kind() == NAMESPACE_DECLARATION)?;
    if declaration.children().find(|child| child.kind() == NAME).as_ref() != Some(&name) {
        return None;
    }
    Some(Prepared {
        range: name.text_range(),
        placeholder: ast::text_of(&name),
        kind: RenameKind::Namespace,
    })
}

/// The symbol a rename is about when the name stands for several: a promoted parameter is a
/// property first.
fn primary_symbol(symbols: &[Symbol]) -> &Symbol {
    symbols
        .iter()
        .find(|symbol| matches!(symbol, Symbol::Property { .. }))
        .unwrap_or(&symbols[0])
}

fn check_declared_in_project(index: &Index, symbols: &[Symbol]) -> Result<(), String> {
    for symbol in symbols {
        if matches!(symbol, Symbol::Variable { .. } | Symbol::Parameter { .. }) {
            continue;
        }
        if let Symbol::Key { kind, .. } = symbol {
            return Err(format!("A {} cannot be renamed", kind.label()));
        }
        let query = Query::new(index, symbol.clone());
        let found = declarations(index, &query);
        if found.is_empty() {
            return Err("The declaration of this name is not known, so it cannot be renamed".to_string());
        }
        if let Some(outside) = found.iter().find(|declaration| declaration.origin != Origin::Project) {
            let place = match outside.origin {
                Origin::Stub => "the PHP standard library",
                _ => "an installed package",
            };
            return Err(format!("This name is declared in {place} and cannot be renamed"));
        }
        if let Symbol::Method { name, .. } = symbol {
            if name.starts_with("__") {
                return Err("Magic methods and constructors cannot be renamed".to_string());
            }
        }
    }
    Ok(())
}

/// Whether `name` can be the name of a thing of this kind, with the reason when it cannot.
pub fn validate_name(kind: RenameKind, name: &str) -> Result<(), String> {
    let identifier = !name.is_empty()
        && name.chars().enumerate().all(|(position, c)| {
            c == '_' || c.is_alphabetic() || (position > 0 && c.is_ascii_digit()) || !c.is_ascii()
        });
    let lower = name.to_ascii_lowercase();
    match kind {
        RenameKind::Namespace => {
            let valid = !name.is_empty()
                && name.split('\\').all(|segment| {
                    !segment.is_empty()
                        && segment.chars().enumerate().all(|(i, c)| {
                            c == '_' || c.is_alphabetic() || (i > 0 && c.is_ascii_digit()) || !c.is_ascii()
                        })
                });
            if !valid {
                return Err(format!("'{name}' is not a valid namespace"));
            }
            Ok(())
        }
        RenameKind::Dataset => {
            if name.is_empty() || name.contains(['\'', '"', '\\', '\n']) {
                return Err(format!("'{name}' is not a valid dataset name"));
            }
            Ok(())
        }
        RenameKind::Key => Err("A string that names a route, a key or a view cannot be renamed".to_string()),
        _ if !identifier => Err(format!("'{name}' is not a valid name")),
        RenameKind::Variable => {
            if SUPERGLOBALS.contains(&name) {
                return Err(format!("'${name}' is reserved"));
            }
            Ok(())
        }
        RenameKind::Property | RenameKind::Method => Ok(()),
        RenameKind::ClassConst => {
            if lower == "class" {
                return Err("'class' cannot be the name of a constant".to_string());
            }
            Ok(())
        }
        RenameKind::Class | RenameKind::Function | RenameKind::Constant => {
            if RESERVED.contains(&lower.as_str()) || RESERVED_TYPES.contains(&lower.as_str()) {
                return Err(format!("'{name}' is a reserved word"));
            }
            Ok(())
        }
    }
}

/// The edits that rename what is under a position.
pub fn rename(
    index: &Index,
    sources: &dyn Sources,
    current: &Current,
    offset: u32,
    new_name: &str,
) -> Result<Rename, String> {
    let prepared = prepare_rename(index, current.root, current.text, offset)?;
    if prepared.kind == RenameKind::Namespace {
        let new_name = new_name.trim_start_matches('$');
        validate_name(prepared.kind, new_name)?;
        return rename_namespace(sources, current, &prepared, new_name);
    }
    let Some((_, symbols)) = symbols_at(index, current.root, offset) else {
        return Err("There is no name to rename here".to_string());
    };
    rename_symbols(index, sources, current, &symbols, prepared.kind, new_name)
}

/// The edits that rename symbols found at a place of the current file. A template asks this with
/// the symbols of its document of PHP.
pub fn rename_symbols(
    index: &Index,
    sources: &dyn Sources,
    current: &Current,
    symbols: &[Symbol],
    kind: RenameKind,
    new_name: &str,
) -> Result<Rename, String> {
    let new_name = if kind == RenameKind::Variable || kind == RenameKind::Property {
        new_name.trim_start_matches('$')
    } else {
        new_name
    };
    validate_name(kind, new_name)?;
    let primary = primary_symbol(symbols).clone();
    if symbol_name(&primary).is_some_and(|old| old == new_name) {
        return Ok(Rename::default());
    }
    for symbol in symbols {
        check_conflicts(index, current, symbol, new_name)?;
    }
    let files = hits_of_symbols(index, sources, current, symbols);
    let mut edits: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
    for file in &files {
        for hit in &file.hits {
            if hit.via_alias
                || hit.kind == crate::refs::HitKind::String && crate::class_strings::kept_by_refactors(&file.path)
            {
                continue;
            }
            let replacement = if hit.dollar {
                format!("${new_name}")
            } else {
                new_name.to_string()
            };
            edits.entry(file.path.clone()).or_default().push(TextEdit {
                range: hit.range,
                text: replacement,
            });
        }
    }
    check_import_clashes(sources, current, &primary, new_name, &edits)?;
    let file_rename = match &primary {
        Symbol::Class(name) => class_file_rename(index, name, new_name),
        _ => None,
    };
    Ok(Rename {
        files: edits
            .into_iter()
            .map(|(path, edits)| FileEdits { path, edits })
            .collect(),
        file_rename,
    })
}

fn symbol_name(symbol: &Symbol) -> Option<&str> {
    match symbol {
        Symbol::Class(name) | Symbol::Function(name) | Symbol::Constant(name) => Some(crate::short(name)),
        Symbol::Method { name, .. }
        | Symbol::Property { name, .. }
        | Symbol::ClassConst { name, .. }
        | Symbol::Parameter { name, .. }
        | Symbol::Variable { name, .. }
        | Symbol::Dataset(name)
        | Symbol::Key { name, .. } => Some(name),
    }
}

fn check_conflicts(index: &Index, current: &Current, symbol: &Symbol, new_name: &str) -> Result<(), String> {
    match symbol {
        Symbol::Class(name) => {
            let target = with_short_name(name, new_name);
            if index
                .class(&target)
                .is_some_and(|found| !found.decl.name.eq_ignore_ascii_case(name))
            {
                return Err(format!("A class named '{target}' already exists"));
            }
        }
        Symbol::Function(name) => {
            let target = with_short_name(name, new_name);
            if index
                .function(&target)
                .is_some_and(|found| !found.decl.name.eq_ignore_ascii_case(name))
            {
                return Err(format!("A function named '{target}' already exists"));
            }
        }
        Symbol::Constant(name) => {
            let target = with_short_name(name, new_name);
            if index.constant(&target).is_some_and(|found| found.decl.name != *name) {
                return Err(format!("A constant named '{target}' already exists"));
            }
        }
        Symbol::Method { .. } | Symbol::Property { .. } | Symbol::ClassConst { .. } => {
            check_member_conflicts(index, symbol, new_name)?;
        }
        Symbol::Variable { scope, name } => {
            let taken = if crate::blade::is_template(current.path) {
                let query = Query::new(
                    index,
                    Symbol::Variable {
                        name: new_name.to_string(),
                        scope: *scope,
                    },
                );
                crate::blade::hits(index, Some(current.path), current.text, &[], &query)
            } else {
                crate::refs::variable_hits(&FileContext::new(index, current.root), *scope, new_name)
            };
            if !taken.is_empty() && name != new_name {
                return Err(format!("A variable named '${new_name}' is already used in this scope"));
            }
        }
        Symbol::Parameter { .. } | Symbol::Key { .. } => {}
        Symbol::Dataset(name) => {
            if name != new_name && !crate::pest::dataset_declarations(index, new_name).is_empty() {
                return Err(format!("A dataset named '{new_name}' already exists"));
            }
        }
    }
    Ok(())
}

fn with_short_name(qualified: &str, short: &str) -> String {
    match qualified.rsplit_once('\\') {
        Some((namespace, _)) => format!("{namespace}\\{short}"),
        None => short.to_string(),
    }
}

/// A member of the same kind with the new name, in the family or anywhere above or below it.
fn check_member_conflicts(index: &Index, symbol: &Symbol, new_name: &str) -> Result<(), String> {
    let query = Query::new(index, symbol.clone());
    let mut related: Vec<&php_index::ClassDecl> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for name in query.family() {
        for ancestor in index.ancestors(&php_index::Type::class(name.clone())) {
            if seen.insert(ancestor.class.decl.name.to_ascii_lowercase()) {
                related.push(ancestor.class.decl);
            }
        }
        for subtype in index.all_subtypes(name) {
            if seen.insert(subtype.decl.name.to_ascii_lowercase()) {
                related.push(subtype.decl);
            }
        }
    }
    for decl in related {
        let clash = match symbol {
            Symbol::Method { .. } => declares_method(decl, new_name),
            Symbol::Property { .. } => declares_property(decl, new_name),
            _ => decl.constant(new_name).is_some(),
        };
        if clash {
            let what = match symbol {
                Symbol::Method { .. } => "method",
                Symbol::Property { .. } => "property",
                _ => "constant",
            };
            return Err(format!("{} already has a {what} named '{new_name}'", decl.name));
        }
    }
    Ok(())
}

/// A file that sees the renamed class by its short name must not already use that name for another
/// class it imports.
fn check_import_clashes(
    sources: &dyn Sources,
    current: &Current,
    symbol: &Symbol,
    new_name: &str,
    edits: &BTreeMap<PathBuf, Vec<TextEdit>>,
) -> Result<(), String> {
    let Symbol::Class(name) = symbol else {
        return Ok(());
    };
    let kind = php_index::UseKind::Class;
    for path in edits.keys() {
        let owned;
        let root = if path == current.path {
            current.root.clone()
        } else {
            let Some(text) = sources.text(path) else {
                continue;
            };
            owned = text;
            parse(&owned).syntax()
        };
        let resolver = php_index::extract::resolver_at(&root, ast::end(&root));
        let clash = resolver.imports(kind).any(|(alias, target)| {
            alias.eq_ignore_ascii_case(new_name) && !target.eq_ignore_ascii_case(name.trim_start_matches('\\'))
        });
        if clash {
            let file = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            return Err(format!("{file} already imports another class as '{new_name}'"));
        }
    }
    Ok(())
}

/// The file of a class that is named after it, and what it is called after the rename.
fn class_file_rename(index: &Index, name: &str, new_name: &str) -> Option<FileRename> {
    let class = index.class(name)?;
    if class.file.origin != Origin::Project {
        return None;
    }
    let old_short = crate::short(&class.decl.name);
    let stem = class.file.path.file_stem()?.to_str()?;
    let extension = class.file.path.extension()?.to_str()?;
    if stem != old_short || class.file.symbols().classes.len() != 1 {
        return None;
    }
    let target = class.file.path.with_file_name(format!("{new_name}.{extension}"));
    (target != class.file.path).then(|| FileRename {
        from: class.file.path.clone(),
        to: target,
        class: class.decl.name.clone(),
    })
}

// Namespaces ------------------------------------------------------------------------------------

/// Renames a namespace: every `namespace` statement that names it exactly, and every written name
/// that has it as the namespace of a class, function or constant: `use` statements, group prefixes
/// and fully qualified names. Names relative to another namespace are not touched.
fn rename_namespace(
    sources: &dyn Sources,
    current: &Current,
    prepared: &Prepared,
    new_name: &str,
) -> Result<Rename, String> {
    let old = prepared.placeholder.trim_matches('\\').to_string();
    let new_name = new_name.trim_matches('\\');
    if old.eq_ignore_ascii_case(new_name) {
        return Ok(Rename::default());
    }
    let word = old.rsplit('\\').next().unwrap_or(&old).to_ascii_lowercase();
    let mut paths = sources.candidates(&word);
    paths.push(current.path.to_path_buf());
    paths.sort();
    paths.dedup();
    let mut edits: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
    for path in paths {
        let found = if path == current.path {
            namespace_edits(current.root, &old, new_name)
        } else {
            let Some(text) = sources.text(&path) else {
                continue;
            };
            namespace_edits(&parse(&text).syntax(), &old, new_name)
        };
        if !found.is_empty() {
            edits.insert(path, found);
        }
    }
    Ok(Rename {
        files: edits
            .into_iter()
            .map(|(path, edits)| FileEdits { path, edits })
            .collect(),
        file_rename: None,
    })
}

fn namespace_edits(root: &SyntaxNode, old: &str, new_name: &str) -> Vec<TextEdit> {
    let mut out = Vec::new();
    for node in root.descendants() {
        match node.kind() {
            NAMESPACE_DECLARATION => {
                if let Some(name) = node.children().find(|child| child.kind() == NAME) {
                    if ast::text_of(&name).trim_matches('\\').eq_ignore_ascii_case(old) {
                        out.push(TextEdit {
                            range: name.text_range(),
                            text: new_name.to_string(),
                        });
                    }
                }
            }
            USE_GROUP => {
                if let Some(prefix) = node.children().find(|child| child.kind() == NAME) {
                    if ast::text_of(&prefix).trim_matches('\\').eq_ignore_ascii_case(old) {
                        out.push(TextEdit {
                            range: prefix.text_range(),
                            text: new_name.to_string(),
                        });
                    }
                }
            }
            NAME => {
                let Some(owner) = node.parent() else {
                    continue;
                };
                if matches!(owner.kind(), NAMESPACE_DECLARATION | USE_GROUP) || is_alias(&node) {
                    continue;
                }
                let written = ast::text_of(&node);
                let Some((prefix, last)) = written.rsplit_once('\\') else {
                    continue;
                };
                let absolute = written.starts_with('\\') || owner.kind() == USE_CLAUSE;
                if absolute && !last.is_empty() && prefix.trim_start_matches('\\').eq_ignore_ascii_case(old) {
                    let start = u32::from(node.text_range().start());
                    let lead = (prefix.len() - prefix.trim_start_matches('\\').len()) as u32;
                    out.push(TextEdit {
                        range: range_of(start + lead, start + prefix.len() as u32),
                        text: new_name.to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|edit| edit.range.start());
    out
}

fn is_alias(name: &SyntaxNode) -> bool {
    name.parent().is_some_and(|owner| owner.kind() == USE_CLAUSE)
        && name
            .siblings_with_tokens(php_syntax::Direction::Prev)
            .any(|element| element.kind() == AS_KW)
}
