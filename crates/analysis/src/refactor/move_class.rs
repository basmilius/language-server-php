//! Move class: a class goes to another namespace, which is another folder of its Composer root,
//! or gets a new name, and everything that names it follows: its own namespace and the names it
//! relies on, every `use` and every reference in the project, and the file itself.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use php_index::composer::Composer;
use php_index::{NameResolver, Origin, UseKind};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange, parse};

use super::draft::Draft;
use super::signature::remove_item;
use super::{Change, Rcx, Refactor, RefactorEnv, RefactorKind};
use crate::actions::edits::{insert, merge_inserts, remove_with_lines, replace};
use crate::ast::{self, child_of, end, start, text_of};
use crate::completion::TextEdit;
use crate::doc_refs::{DocItemKind, doc_items};
use crate::imports::import_edit;
use crate::references::{Current, hits_of_symbols};
use crate::refs::{HitKind, Symbol};

/// How many namespaces a cursor on a class is offered to move it to.
const MAX_TARGETS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassMove {
    pub old: String,
    pub new: String,
    pub from: PathBuf,
    pub to: PathBuf,
}

pub(crate) fn namespace_of(name: &str) -> &str {
    name.rsplit_once('\\').map_or("", |(namespace, _)| namespace)
}

pub(crate) fn short_of(name: &str) -> &str {
    name.rsplit('\\').next().unwrap_or(name)
}

// PSR-4 -----------------------------------------------------------------------------------------

/// The class a path stands for under the PSR-4 maps of a project, with the folder it is read from.
pub(crate) fn psr4_class(composer: &Composer, path: &Path) -> Option<(String, PathBuf)> {
    let mut best: Option<(&String, &PathBuf)> = None;
    for (prefix, dir) in &composer.autoload.psr4 {
        if path.starts_with(dir) && best.is_none_or(|(_, held)| dir.components().count() > held.components().count()) {
            best = Some((prefix, dir));
        }
    }
    let (prefix, dir) = best?;
    let relative = path.strip_prefix(dir).ok()?.with_extension("");
    let rest: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if rest.is_empty() || path.extension().is_none_or(|extension| extension != "php") {
        return None;
    }
    Some((format!("{prefix}{}", rest.join("\\")), dir.clone()))
}

/// The file a class has under the PSR-4 map that holds a place, or under the first one that fits.
pub(crate) fn psr4_path(composer: &Composer, class: &str, near: &Path) -> Option<PathBuf> {
    let mut fitting: Vec<(&String, &PathBuf)> = composer
        .autoload
        .psr4
        .iter()
        .filter(|(prefix, _)| class.starts_with(prefix.as_str()) && class.len() > prefix.len())
        .map(|(prefix, dir)| (prefix, dir))
        .collect();
    fitting.sort_by_key(|(prefix, dir)| (!near.starts_with(dir), std::cmp::Reverse(prefix.len())));
    let (prefix, dir) = fitting.first()?;
    let rest = class[prefix.len()..].replace('\\', "/");
    Some(dir.join(format!("{rest}.php")))
}

// Reading a file for what names it uses ---------------------------------------------------------

/// A name written in a file, with what it names.
struct NameUse {
    start: u32,
    end: u32,
    written: String,
    kind: UseKind,
    in_doc: bool,
}

fn name_kind(name: &SyntaxNode) -> Option<UseKind> {
    let parent = name.parent()?;
    let first = parent.children().next().as_ref() == Some(name);
    match parent.kind() {
        NEW_EXPR | NAMED_TYPE | ATTRIBUTE | TRAIT_USE => Some(UseKind::Class),
        SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR if first => Some(UseKind::Class),
        BINARY_EXPR if !first && ast::has_token(&parent, INSTANCEOF_KW) => Some(UseKind::Class),
        CALL_EXPR if first => Some(UseKind::Function),
        _ if super::exprs::is_expression(name) => {
            let written = text_of(name).to_ascii_lowercase();
            (!matches!(written.as_str(), "true" | "false" | "null")).then_some(UseKind::Constant)
        }
        _ => None,
    }
}

fn names_in(root: &SyntaxNode) -> Vec<NameUse> {
    let mut out = Vec::new();
    for node in root.descendants() {
        if node.kind() == NAME {
            let written = text_of(&node);
            let lower = written.to_ascii_lowercase();
            if matches!(lower.as_str(), "self" | "static" | "parent")
                || crate::inspections::util::is_builtin_type(&lower)
            {
                continue;
            }
            if let Some(kind) = name_kind(&node) {
                out.push(NameUse {
                    start: start(&node),
                    end: end(&node),
                    written,
                    kind,
                    in_doc: false,
                });
            }
        }
    }
    for element in root.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if token.kind() != DOC_COMMENT {
            continue;
        }
        let base = u32::from(token.text_range().start());
        for item in doc_items(token.text(), base) {
            if let DocItemKind::Class(name) = item.kind {
                let lower = name.to_ascii_lowercase();
                if crate::inspections::util::is_builtin_type(&lower)
                    || matches!(lower.as_str(), "self" | "static" | "parent" | "$this")
                {
                    continue;
                }
                out.push(NameUse {
                    start: item.start,
                    end: item.end,
                    written: name,
                    kind: UseKind::Class,
                    in_doc: true,
                });
            }
        }
    }
    out.sort_by_key(|used| used.start);
    out
}

// Relocating -------------------------------------------------------------------------------------

struct Loaded {
    path: PathBuf,
    text: String,
    root: SyntaxNode,
}

fn load(renv: &RefactorEnv<'_>, path: &Path) -> Result<Loaded, String> {
    if path == renv.path {
        return Ok(Loaded {
            path: path.to_path_buf(),
            text: renv.env.text.to_string(),
            root: renv.env.root.clone(),
        });
    }
    let text = renv
        .sources
        .text(path)
        .ok_or_else(|| format!("{} cannot be read", path.display()))?;
    let root = parse(&text).syntax();
    Ok(Loaded {
        path: path.to_path_buf(),
        text,
        root,
    })
}

/// The one class-like a file declares, and the namespace statement around it.
fn declaration_of(file: &Loaded) -> Result<(SyntaxNode, Option<SyntaxNode>), String> {
    let classes: Vec<SyntaxNode> = file
        .root
        .descendants()
        .filter(|node| {
            matches!(
                node.kind(),
                CLASS_DECLARATION | INTERFACE_DECLARATION | TRAIT_DECLARATION | ENUM_DECLARATION
            )
        })
        .collect();
    let [class] = classes.as_slice() else {
        return Err(format!("{} does not declare exactly one class", file.path.display()));
    };
    let namespaces: Vec<SyntaxNode> = file
        .root
        .descendants()
        .filter(|node| node.kind() == NAMESPACE_DECLARATION)
        .collect();
    if namespaces.len() > 1 {
        return Err(format!("{} has more than one namespace", file.path.display()));
    }
    Ok((class.clone(), namespaces.into_iter().next()))
}

/// How a written name is spelled to name `target` from where `resolver` reads it.
struct Spelling<'a> {
    renv: &'a RefactorEnv<'a>,
    resolver: &'a NameResolver,
    /// The classes that are on their way somewhere else, in lowercase.
    leaving: &'a HashSet<String>,
}

impl Spelling<'_> {
    /// Whether the short name of a class is free to be imported where the resolver reads.
    fn short_is_free(&self, target: &str) -> bool {
        let short = short_of(target);
        if self
            .resolver
            .imports(UseKind::Class)
            .any(|(alias, imported)| alias.eq_ignore_ascii_case(short) && !imported.eq_ignore_ascii_case(target))
        {
            return false;
        }
        let local = self.resolver.qualify(short);
        self.leaving.contains(&local.to_ascii_lowercase())
            || !self
                .renv
                .env
                .index
                .class(&local)
                .is_some_and(|found| !found.decl.name.eq_ignore_ascii_case(target))
    }
}

/// Everything a set of moves changes.
pub(crate) fn relocate(renv: &RefactorEnv<'_>, moves: &[ClassMove], perform: bool) -> Result<Change, String> {
    let index = renv.env.index;
    let by_old: HashMap<String, &ClassMove> = moves
        .iter()
        .map(|moved| (moved.old.to_ascii_lowercase(), moved))
        .collect();
    let target_of = |fqn: &str| -> String {
        by_old
            .get(&fqn.trim_start_matches('\\').to_ascii_lowercase())
            .map_or_else(|| fqn.trim_start_matches('\\').to_string(), |moved| moved.new.clone())
    };
    for moved in moves {
        if moved.old == moved.new {
            continue;
        }
        if index.class(&moved.new).is_some_and(|found| {
            !found.decl.name.eq_ignore_ascii_case(&moved.old) && !by_old.contains_key(&moved.new.to_ascii_lowercase())
        }) {
            return Err(format!("A class named '{}' already exists", moved.new));
        }
        if moved.to != moved.from && moved.to.exists() {
            return Err(format!("{} exists already", moved.to.display()));
        }
    }
    let mut draft = Draft::new(renv);
    let moved_paths: HashSet<&Path> = moves.iter().map(|moved| moved.from.as_path()).collect();
    let leaving: HashSet<String> = moves.iter().map(|moved| moved.old.to_ascii_lowercase()).collect();
    for moved in moves {
        let file = load(renv, &moved.from)?;
        edit_moved_file(renv, &mut draft, &file, moved, &target_of, &leaving)?;
        if perform && moved.from != moved.to {
            draft.move_file(moved.from.clone(), moved.to.clone());
        }
    }
    edit_references(renv, &mut draft, moves, &moved_paths, &leaving)?;
    draft.finish()
}

fn edit_moved_file(
    renv: &RefactorEnv<'_>,
    draft: &mut Draft<'_>,
    file: &Loaded,
    moved: &ClassMove,
    target_of: &dyn Fn(&str) -> String,
    leaving: &HashSet<String>,
) -> Result<(), String> {
    let (class, namespace_node) = declaration_of(file)?;
    let (old_namespace, new_namespace) = (namespace_of(&moved.old), namespace_of(&moved.new));
    let mut edits: Vec<TextEdit> = Vec::new();
    // The namespace statement.
    if !old_namespace.eq_ignore_ascii_case(new_namespace) {
        match (&namespace_node, new_namespace.is_empty()) {
            (Some(node), true) => {
                if ast::has_token(node, LBRACE) {
                    return Err("A class in a block namespace cannot move to the global one".to_string());
                }
                let semicolon = ast::tokens(node)
                    .find(|token| token.kind() == SEMICOLON)
                    .ok_or("The namespace has no end")?;
                let header = TextRange::new(node.text_range().start(), semicolon.text_range().end());
                edits.push(remove_with_lines(&file.text, header));
            }
            (Some(node), false) => {
                let name = child_of(node, NAME).ok_or("The namespace has no name")?;
                edits.push(replace(name.text_range(), new_namespace));
            }
            (None, false) => edits.push(insert(
                namespace_offset(file),
                format!("namespace {new_namespace};\n\n"),
            )),
            (None, true) => {}
        }
    }
    // The name of the class.
    if !short_of(&moved.old).eq(short_of(&moved.new)) {
        let name = child_of(&class, NAME).ok_or("The class has no name")?;
        edits.push(replace(name.text_range(), short_of(&moved.new)));
    }
    // The names the file relies on.
    let class_start = start(&class);
    let old_resolver = php_index::extract::resolver_at(&file.root, class_start);
    let mut new_resolver = old_resolver.clone();
    new_resolver.namespace = new_namespace.to_string();
    let spelling = Spelling {
        renv,
        resolver: &new_resolver,
        leaving,
    };
    let mut imports: Vec<(String, UseKind)> = Vec::new();
    for used in names_in(&file.root) {
        if let Some(edit) = respell(renv, file, &new_resolver, &spelling, &used, target_of, &mut imports) {
            edits.push(edit);
        }
    }
    let mut import_edits = Vec::new();
    for (fqn, kind) in imports {
        if let Some(edit) = import_edit(&file.text, &file.root, class_start, &fqn, kind) {
            import_edits.push(edit);
        }
    }
    edits.extend(merge_inserts(import_edits));
    draft.edits(&file.path, edits);
    Ok(())
}

/// Where a `namespace` statement goes in a file that has none: after the open tag and `declare`.
fn namespace_offset(file: &Loaded) -> u32 {
    let mut at = file
        .root
        .children_with_tokens()
        .filter_map(php_syntax::SyntaxElement::into_token)
        .find(|token| token.kind() == OPEN_TAG)
        .map_or(0, |token| u32::from(token.text_range().end()));
    for statement in file.root.children() {
        if statement.kind() == DECLARE_STATEMENT {
            at = crate::actions::edits::line_end(&file.text, end(&statement) as usize) as u32;
        } else {
            break;
        }
    }
    let rest = &file.text[at as usize..];
    if rest.starts_with('\n') && at > 0 && file.text[..at as usize].ends_with("?php\n") {
        return at + 1;
    }
    at
}

/// The edit that keeps a name in the moved file naming what it named.
#[allow(clippy::too_many_arguments)]
fn respell(
    renv: &RefactorEnv<'_>,
    file: &Loaded,
    new_resolver: &NameResolver,
    spelling: &Spelling<'_>,
    used: &NameUse,
    target_of: &dyn Fn(&str) -> String,
    imports: &mut Vec<(String, UseKind)>,
) -> Option<TextEdit> {
    let index = renv.env.index;
    let written = used.written.as_str();
    let range = TextRange::new(used.start.into(), used.end.into());
    let resolver_here = php_index::extract::resolver_at(&file.root, used.start);
    match used.kind {
        UseKind::Class => {
            let old_target = resolver_here.resolve_class(written);
            let target = target_of(&old_target);
            let mut new_here = resolver_here.clone();
            new_here.namespace = new_resolver.namespace.clone();
            let new_target = new_here.resolve_class(written);
            let renamed = !short_of(&old_target).eq(short_of(&target));
            if written.starts_with('\\') {
                return (!target.eq(&old_target)).then(|| replace(range, format!("\\{target}")));
            }
            if new_target.eq_ignore_ascii_case(&target) {
                if renamed && !written.contains('\\') {
                    return Some(replace(range, short_of(&target)));
                }
                if renamed {
                    return Some(replace(range, format!("\\{target}")));
                }
                return None;
            }
            if written.contains('\\') || used.in_doc && !spelling.short_is_free(&target) {
                return Some(replace(range, format!("\\{target}")));
            }
            if !spelling.short_is_free(&target) {
                return Some(replace(range, format!("\\{target}")));
            }
            if !imports
                .iter()
                .any(|(known, kind)| known.eq_ignore_ascii_case(&target) && *kind == UseKind::Class)
            {
                imports.push((target.clone(), UseKind::Class));
            }
            renamed.then(|| replace(range, short_of(&target)))
        }
        UseKind::Function | UseKind::Constant => {
            if written.starts_with('\\') || written.contains('\\') {
                return None;
            }
            let candidates = |resolver: &NameResolver| match used.kind {
                UseKind::Function => resolver.function_candidates(written),
                _ => resolver.constant_candidates(written),
            };
            let exists = |name: &String| match used.kind {
                UseKind::Function => index.function(name).is_some(),
                _ => index.constant(name).is_some(),
            };
            let old_target = candidates(&resolver_here).into_iter().find(|name| exists(name))?;
            let mut new_here = resolver_here.clone();
            new_here.namespace = new_resolver.namespace.clone();
            let new_target = candidates(&new_here).into_iter().find(|name| exists(name));
            if new_target.as_deref() == Some(old_target.as_str()) || !old_target.contains('\\') {
                return None;
            }
            if !imports
                .iter()
                .any(|(known, kind)| known.eq_ignore_ascii_case(&old_target) && *kind == used.kind)
            {
                imports.push((old_target, used.kind));
            }
            None
        }
    }
}

/// The names in other files that stand for a moved class.
fn edit_references(
    renv: &RefactorEnv<'_>,
    draft: &mut Draft<'_>,
    moves: &[ClassMove],
    moved_paths: &HashSet<&Path>,
    leaving: &HashSet<String>,
) -> Result<(), String> {
    let index = renv.env.index;
    let current = Current {
        path: renv.path,
        text: renv.env.text,
        root: renv.env.root,
    };
    let symbols: Vec<Symbol> = moves.iter().map(|moved| Symbol::Class(moved.old.clone())).collect();
    let found = hits_of_symbols(index, renv.sources, &current, &symbols);
    for file_hits in found {
        if moved_paths.contains(file_hits.path.as_path()) {
            continue;
        }
        let file = load(renv, &file_hits.path)?;
        let mut edits: Vec<TextEdit> = Vec::new();
        let mut imports: Vec<(String, Option<String>)> = Vec::new();
        let mut seen: HashSet<(u32, u32)> = HashSet::new();
        let anchor = file_hits.hits.first().map_or(0, |hit| u32::from(hit.range.start()));
        for hit in &file_hits.hits {
            let Symbol::Class(old) = &hit.symbol else {
                continue;
            };
            let Some(moved) = moves.iter().find(|moved| moved.old.eq_ignore_ascii_case(old)) else {
                continue;
            };
            if hit.kind == HitKind::Declaration || hit.via_alias {
                continue;
            }
            let hit_start = u32::from(hit.range.start());
            let hit_end = u32::from(hit.range.end());
            if !seen.insert((hit_start, hit_end)) {
                continue;
            }
            let renamed = !short_of(&moved.old).eq(short_of(&moved.new));
            let new_namespace = namespace_of(&moved.new);
            let left = written_start(&file.text, hit_start);
            let written = &file.text[left as usize..hit_end as usize];
            let whole = TextRange::new(left.into(), hit_end.into());
            if hit.kind == HitKind::Import {
                if let Some(edit) = import_edits(&file, hit.range, moved, &mut imports) {
                    edits.extend(edit);
                }
                continue;
            }
            if written.starts_with('\\') || written.contains('\\') {
                edits.push(replace(whole, format!("\\{}", moved.new)));
                continue;
            }
            let resolver = php_index::extract::resolver_at(&file.root, hit_start);
            let via_import = resolver
                .imports(UseKind::Class)
                .any(|(alias, _)| alias.eq_ignore_ascii_case(written));
            if via_import {
                if renamed {
                    edits.push(replace(whole, short_of(&moved.new)));
                }
                continue;
            }
            if resolver.namespace.eq_ignore_ascii_case(new_namespace) {
                if renamed {
                    edits.push(replace(whole, short_of(&moved.new)));
                }
                continue;
            }
            let spelling = Spelling {
                renv,
                resolver: &resolver,
                leaving,
            };
            if !spelling.short_is_free(&moved.new) {
                edits.push(replace(whole, format!("\\{}", moved.new)));
                continue;
            }
            if !imports.iter().any(|(known, _)| known.eq_ignore_ascii_case(&moved.new)) {
                imports.push((moved.new.clone(), None));
            }
            if renamed {
                edits.push(replace(whole, short_of(&moved.new)));
            }
        }
        let mut insertions = Vec::new();
        for (fqn, alias) in imports {
            match alias {
                None => {
                    if let Some(edit) = import_edit(&file.text, &file.root, anchor, &fqn, UseKind::Class) {
                        insertions.push(edit);
                    }
                }
                Some(alias) => {
                    if let Some(edit) = import_edit(&file.text, &file.root, anchor, &fqn, UseKind::Class) {
                        insertions.push(TextEdit {
                            new_text: edit.new_text.replacen(
                                &format!("use {fqn};"),
                                &format!("use {fqn} as {alias};"),
                                1,
                            ),
                            ..edit
                        });
                    }
                }
            }
        }
        edits.extend(merge_inserts(insertions));
        draft.edits(&file.path, edits);
    }
    Ok(())
}

/// The start of a written name whose last segment starts at an offset.
fn written_start(text: &str, last_segment: u32) -> u32 {
    let bytes = text.as_bytes();
    let mut at = last_segment as usize;
    while at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_' || bytes[at - 1] == b'\\') {
        at -= 1;
    }
    at as u32
}

/// The edits of an import of a moved class: the clause points at the new name, or leaves its
/// group for a statement of its own.
fn import_edits(
    file: &Loaded,
    range: TextRange,
    moved: &ClassMove,
    imports: &mut Vec<(String, Option<String>)>,
) -> Option<Vec<TextEdit>> {
    let name = file
        .root
        .covering_element(range)
        .into_token()?
        .parent()
        .filter(|parent| parent.kind() == NAME)?;
    let clause = name.parent().filter(|parent| parent.kind() == USE_CLAUSE)?;
    let group = clause.parent().filter(|parent| parent.kind() == USE_GROUP);
    let has_alias = clause
        .descendants_with_tokens()
        .filter_map(php_syntax::SyntaxElement::into_token)
        .any(|token| token.kind() == AS_KW);
    let alias = has_alias
        .then(|| {
            clause
                .descendants_with_tokens()
                .filter_map(php_syntax::SyntaxElement::into_token)
                .skip_while(|token| token.kind() != AS_KW)
                .find(|token| token.kind() == IDENT)
                .map(|token| token.text().to_string())
        })
        .flatten();
    match group {
        None => {
            let leading = if text_of(&name).starts_with('\\') { "\\" } else { "" };
            Some(vec![replace(name.text_range(), format!("{leading}{}", moved.new))])
        }
        Some(group) => {
            if namespace_of(&moved.old).eq_ignore_ascii_case(namespace_of(&moved.new)) {
                return Some(vec![replace(name.text_range(), short_of(&moved.new))]);
            }
            let statement = group.parent().filter(|parent| parent.kind() == USE_STATEMENT)?;
            let clauses: Vec<SyntaxNode> = group.children().filter(|child| child.kind() == USE_CLAUSE).collect();
            let position = clauses.iter().position(|candidate| *candidate == clause)?;
            imports.push((moved.new.clone(), alias));
            if clauses.len() == 1 {
                return Some(vec![remove_with_lines(&file.text, statement.text_range())]);
            }
            Some(vec![remove_item(&group, position, USE_CLAUSE)?])
        }
    }
}

// Offering it -----------------------------------------------------------------------------------

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let cx = &rcx.cx;
    let Some(composer) = rcx.renv.composer else {
        return;
    };
    if !rcx.range.is_empty() {
        return;
    }
    let offset = u32::from(rcx.range.start());
    let Some(token) = super::exprs::token_near(&cx.root, offset) else {
        return;
    };
    let Some(class) = token
        .parent()
        .filter(|parent| parent.kind() == NAME)
        .and_then(|name| name.parent())
        .filter(|declaration| {
            matches!(
                declaration.kind(),
                CLASS_DECLARATION | INTERFACE_DECLARATION | TRAIT_DECLARATION | ENUM_DECLARATION
            )
        })
    else {
        return;
    };
    let file = Loaded {
        path: rcx.renv.path.to_path_buf(),
        text: cx.text.to_string(),
        root: cx.root.clone(),
    };
    if declaration_of(&file).is_err() {
        return;
    }
    let Some(name) = child_of(&class, NAME) else {
        return;
    };
    let analyzer = cx.file.analyzer(&class);
    let old = analyzer.resolver.qualify(&text_of(&name));
    if cx
        .index
        .class(&old)
        .is_none_or(|found| found.file.origin != Origin::Project)
    {
        return;
    }
    let Some((expected, dir)) = psr4_class(composer, rcx.renv.path) else {
        return;
    };
    let at = start(&class);
    if !expected.eq_ignore_ascii_case(&old) {
        // The file and the namespace disagree: one of them follows the other.
        if expected.contains('\\') || !expected.is_empty() {
            let new = expected.clone();
            let from = rcx.renv.path.to_path_buf();
            let title = format!("Change the class to '{new}', as its file says");
            let moved = ClassMove {
                old: old.clone(),
                new,
                from: from.clone(),
                to: from,
            };
            out.push(Refactor::new(
                format!("move-class-namespace@{at}"),
                title,
                RefactorKind::Move,
                true,
                move || relocate(rcx.renv, std::slice::from_ref(&moved), true),
            ));
        }
        if let Some(path) = psr4_path(composer, &old, rcx.renv.path).filter(|path| path != rcx.renv.path) {
            let moved = ClassMove {
                old: old.clone(),
                new: old.clone(),
                from: rcx.renv.path.to_path_buf(),
                to: path.clone(),
            };
            let title = format!("Move the file to {}", relative(&path, &composer.root));
            out.push(Refactor::new(
                format!("move-class-file@{at}"),
                title,
                RefactorKind::Move,
                true,
                move || relocate(rcx.renv, std::slice::from_ref(&moved), true),
            ));
        }
        return;
    }
    let current_namespace = namespace_of(&old).to_string();
    for target in target_namespaces(rcx, composer, &dir, &old, &current_namespace) {
        let new = if target.is_empty() {
            short_of(&old).to_string()
        } else {
            format!("{target}\\{}", short_of(&old))
        };
        let Some(path) = psr4_path(composer, &new, rcx.renv.path) else {
            continue;
        };
        let moved = ClassMove {
            old: old.clone(),
            new,
            from: rcx.renv.path.to_path_buf(),
            to: path,
        };
        let title = format!("Move class to namespace {target}");
        out.push(Refactor::new(
            format!("move-class@{at}:{target}"),
            title,
            RefactorKind::Move,
            true,
            move || relocate(rcx.renv, std::slice::from_ref(&moved), true),
        ));
    }
}

fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root).unwrap_or(path).to_string_lossy().into_owned()
}

/// The namespaces of the project's classes that a class may move to, the nearest first.
fn target_namespaces(rcx: &Rcx<'_>, composer: &Composer, dir: &Path, old: &str, current: &str) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for class in rcx.cx.index.class_names() {
        if class.file.origin != Origin::Project || !class.file.path.starts_with(dir) {
            continue;
        }
        let namespace = namespace_of(&class.summary.name).to_string();
        if namespace.eq_ignore_ascii_case(current)
            || namespace.is_empty() && composer.autoload.psr4.iter().all(|(prefix, _)| !prefix.is_empty())
        {
            continue;
        }
        *counts.entry(namespace).or_default() += 1;
    }
    let shared = |namespace: &str| {
        current
            .split('\\')
            .zip(namespace.split('\\'))
            .take_while(|(left, right)| left.eq_ignore_ascii_case(right))
            .count()
    };
    let mut all: Vec<(String, usize)> = counts.into_iter().collect();
    all.retain(|(namespace, _)| {
        let candidate = if namespace.is_empty() {
            short_of(old).to_string()
        } else {
            format!("{namespace}\\{}", short_of(old))
        };
        rcx.cx.index.class(&candidate).is_none() && psr4_path(composer, &candidate, rcx.renv.path).is_some()
    });
    all.sort_by_key(|(namespace, count)| {
        (
            std::cmp::Reverse(shared(namespace)),
            std::cmp::Reverse(*count),
            namespace.clone(),
        )
    });
    all.into_iter()
        .take(MAX_TARGETS)
        .map(|(namespace, _)| namespace)
        .collect()
}

// Following a renamed file ------------------------------------------------------------------------

/// The edits that follow files being renamed or moved: the namespace, the name and every
/// reference of each class that sat where its Composer map says and still does at its new place.
/// Nothing for a file that never followed the map, or that goes where no map reaches.
pub fn move_files(
    renv: &RefactorEnv<'_>,
    pairs: &[(PathBuf, PathBuf)],
    perform: bool,
) -> Result<Option<Change>, String> {
    let Some(composer) = renv.composer else {
        return Ok(None);
    };
    let index = renv.env.index;
    let mut moves: Vec<ClassMove> = Vec::new();
    for (from, to) in pairs {
        if from.extension().is_none_or(|extension| extension != "php")
            || to.extension().is_none_or(|extension| extension != "php")
        {
            continue;
        }
        let Some(entry) = index.file(from).filter(|entry| entry.origin == Origin::Project) else {
            continue;
        };
        let symbols = entry.symbols();
        let [class] = symbols.classes.as_slice() else {
            continue;
        };
        let Some((expected, _)) = psr4_class(composer, from) else {
            continue;
        };
        if !expected.eq_ignore_ascii_case(&class.name) {
            continue;
        }
        let Some((new, _)) = psr4_class(composer, to) else {
            continue;
        };
        if new.eq_ignore_ascii_case(&class.name) {
            continue;
        }
        moves.push(ClassMove {
            old: class.name.clone(),
            new,
            from: from.clone(),
            to: to.clone(),
        });
    }
    if moves.is_empty() {
        return Ok(None);
    }
    relocate(renv, &moves, perform).map(Some)
}
