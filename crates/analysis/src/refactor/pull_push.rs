//! Pull members up and push them down: a method or a property moves between a class and its
//! parent, or a method is declared in an interface or an abstract class above it, or it moves into
//! the classes below. What a member needs from its class must be there at the new place, and a
//! place in a package is never changed.

use php_index::{ClassKind, Origin, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange, parse};

use super::draft::Draft;
use super::exprs::{text_slice, token_near};
use super::extract_member::{MemberGroup, is_constructor, member_insertion};
use super::names::fully_qualified;
use super::signature::Loaded;
use super::{Change, Rcx, Refactor, RefactorKind};
use crate::actions::edits::indent_of;
use crate::actions::imports::ClassWriter;
use crate::actions::members::style_of;
use crate::ast::{self, child_of, end, first_token, has_token, start, text_of};
use crate::completion::TextEdit;
use crate::doc_refs::{DocItemKind, doc_items};
use crate::inspections::{Cx, InspectionEnv};
use crate::references::{Current, hits_of_symbols};
use crate::refs::Symbol;

/// A member that can move: a method, or a property declared on its own.
struct Member {
    node: SyntaxNode,
    name: String,
    is_method: bool,
    class: SyntaxNode,
}

fn member_at(rcx: &Rcx<'_>) -> Option<Member> {
    if !rcx.range.is_empty() {
        return None;
    }
    let cx = &rcx.cx;
    let offset = u32::from(rcx.range.start());
    let token = token_near(&cx.root, offset)?;
    let node = token
        .parent()?
        .ancestors()
        .find(|node| matches!(node.kind(), METHOD_DECLARATION | PROPERTY_DECLARATION))?;
    let in_header = match node.kind() {
        METHOD_DECLARATION => child_of(&node, BLOCK).is_none_or(|body| offset <= start(&body)),
        _ => true,
    };
    if !in_header || node.parent().is_none_or(|parent| parent.kind() != CLASS_BODY) {
        return None;
    }
    let class = node
        .parent()?
        .parent()
        .filter(|class| ast::is_class_like(class.kind()))?;
    let (name, is_method) = match node.kind() {
        METHOD_DECLARATION => {
            let name = text_of(&child_of(&node, NAME)?);
            if is_constructor(&node) || name.starts_with("__") {
                return None;
            }
            (name, true)
        }
        _ => {
            let elements: Vec<SyntaxNode> = node
                .children()
                .filter(|child| child.kind() == PROPERTY_ELEMENT)
                .collect();
            let [element] = elements.as_slice() else {
                return None;
            };
            (
                first_token(element, VARIABLE)?
                    .text()
                    .trim_start_matches('$')
                    .to_string(),
                false,
            )
        }
    };
    Some(Member {
        node,
        name,
        is_method,
        class,
    })
}

fn visibility_of(member: &SyntaxNode) -> &'static str {
    match child_of(member, MODIFIER_LIST) {
        Some(list) if has_token(&list, PRIVATE_KW) => "private",
        Some(list) if has_token(&list, PROTECTED_KW) => "protected",
        _ => "public",
    }
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let Some(member) = member_at(rcx) else {
        return;
    };
    let cx = &rcx.cx;
    let analyzer = cx.file.analyzer(&member.class);
    let Some(class_name) = child_of(&member.class, NAME).map(|name| analyzer.resolver.qualify(&text_of(&name))) else {
        return;
    };
    let Some(found) = cx.index.class(&class_name) else {
        return;
    };
    let at = start(&member.node);
    let kind = found.decl.kind;
    if kind == ClassKind::Class || kind == ClassKind::Trait {
        // Up to the parent, or declared in what is above.
        if let Some(parent) = found
            .decl
            .extends
            .first()
            .and_then(|parent| parent.class_names().first().map(|name| (*name).to_string()))
        {
            if kind == ClassKind::Class {
                let title = format!("Pull {} up to {}", noun(&member), crate::short(&parent));
                let target = parent.clone();
                let moving = class_name.clone();
                out.push(Refactor::new(
                    format!("pull-up@{at}"),
                    title,
                    RefactorKind::Move,
                    true,
                    move || pull_up(rcx, &moving, &target, Pull::Move),
                ));
                if member.is_method && cx.index.class(&parent).is_some_and(|above| above.decl.is_abstract) {
                    let title = format!("Declare {} in {}", member.name, crate::short(&parent));
                    let target = parent.clone();
                    let declaring = class_name.clone();
                    out.push(Refactor::new(
                        format!("declare-abstract@{at}"),
                        title,
                        RefactorKind::Move,
                        true,
                        move || pull_up(rcx, &declaring, &target, Pull::Declare),
                    ));
                }
            }
        }
        if member.is_method {
            for interface in &found.decl.implements {
                let Some(name) = interface.class_names().first().map(|name| (*name).to_string()) else {
                    continue;
                };
                let title = format!("Declare {} in interface {}", member.name, crate::short(&name));
                let declaring = class_name.clone();
                out.push(Refactor::new(
                    format!("declare-interface@{at}:{name}"),
                    title,
                    RefactorKind::Move,
                    true,
                    move || pull_up(rcx, &declaring, &name, Pull::Declare),
                ));
            }
        }
    }
    if kind == ClassKind::Class && !cx.index.direct_subtypes(&class_name).is_empty() {
        let count = cx.index.direct_subtypes(&class_name).len();
        let title = format!(
            "Push {} down to {}",
            noun(&member),
            if count == 1 {
                "the subclass".to_string()
            } else {
                format!("{count} subclasses")
            }
        );
        let pushing = class_name.clone();
        out.push(Refactor::new(
            format!("push-down@{at}"),
            title,
            RefactorKind::Move,
            true,
            move || push_down(rcx, &pushing),
        ));
    }
}

fn noun(member: &Member) -> String {
    if member.is_method {
        format!("method {}", member.name)
    } else {
        format!("property ${}", member.name)
    }
}

enum Pull {
    /// The member goes to the class above.
    Move,
    /// The class above only declares it, and the member stays.
    Declare,
}

/// A class of the index as the file that declares it, and its declaration there.
fn declaration_in(rcx: &Rcx<'_>, class: &str) -> Result<(Loaded, SyntaxNode), String> {
    let cx = &rcx.cx;
    let found = cx.index.class(class).ok_or_else(|| format!("{class} is not known"))?;
    if found.file.origin != Origin::Project {
        return Err(format!(
            "{class} is in a package or the standard library, which is not changed"
        ));
    }
    let path = found.file.path.clone();
    let file = if path == rcx.renv.path {
        Loaded {
            path,
            text: cx.text.to_string(),
            root: cx.root.clone(),
        }
    } else {
        let text = rcx.renv.sources.text(&path).ok_or("A file cannot be read")?;
        Loaded {
            root: parse(&text).syntax(),
            text,
            path,
        }
    };
    let name = file
        .root
        .covering_element(TextRange::new(
            found.decl.name_span.start.into(),
            found.decl.name_span.end.into(),
        ))
        .into_token()
        .and_then(|token| token.parent())
        .and_then(|name| name.parent())
        .filter(|node| ast::is_class_like(node.kind()))
        .ok_or("The class cannot be found in its file")?;
    Ok((file, name))
}

/// Runs something with the context of a file, which is the open one or one of the project's.
fn with_cx<R>(rcx: &Rcx<'_>, file: &Loaded, run: impl FnOnce(&Cx) -> R) -> R {
    if file.path == rcx.renv.path {
        return run(&rcx.cx);
    }
    let env = InspectionEnv {
        index: rcx.renv.env.index,
        text: &file.text,
        root: &file.root,
        settings: rcx.renv.env.settings,
        ready: rcx.renv.env.ready,
        externals: rcx.renv.env.externals,
    };
    let cx = Cx::new(&env);
    run(&cx)
}

/// What a member takes from the class around it: the names it reads through `$this`, `self` and `static`.
fn needs_of(member: &SyntaxNode) -> Vec<(String, Need)> {
    let mut out: Vec<(String, Need)> = Vec::new();
    for node in member.descendants() {
        match node.kind() {
            PROPERTY_FETCH_EXPR => {
                let Some(receiver) = node.children().next() else {
                    continue;
                };
                if text_of(&receiver) != "$this" {
                    continue;
                }
                let Some(name) = node.children().filter(|child| child.kind() == NAME).last() else {
                    continue;
                };
                let is_call = node.parent().is_some_and(|parent| {
                    parent.kind() == CALL_EXPR && parent.children().next().as_ref() == Some(&node)
                });
                out.push((text_of(&name), if is_call { Need::Method } else { Need::Property }));
            }
            SCOPED_ACCESS_EXPR => {
                let Some(class) = node.children().next() else {
                    continue;
                };
                let written = text_of(&class).to_ascii_lowercase();
                if !matches!(written.as_str(), "self" | "static") {
                    continue;
                }
                let Some(name) = node.children().filter(|child| child.kind() == NAME).last() else {
                    continue;
                };
                let is_call = node.parent().is_some_and(|parent| {
                    parent.kind() == CALL_EXPR && parent.children().next().as_ref() == Some(&node)
                });
                if text_of(&name).eq_ignore_ascii_case("class") {
                    continue;
                }
                out.push((text_of(&name), if is_call { Need::Method } else { Need::Constant }));
            }
            STATIC_PROPERTY_EXPR => {
                let Some(class) = node.children().next() else {
                    continue;
                };
                if !matches!(text_of(&class).to_ascii_lowercase().as_str(), "self" | "static") {
                    continue;
                }
                if let Some(name) = node.children().nth(1) {
                    out.push((text_of(&name).trim_start_matches('$').to_string(), Need::Property));
                }
            }
            _ => {}
        }
    }
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    Method,
    Property,
    Constant,
}

fn uses_parent(member: &SyntaxNode) -> bool {
    member.descendants().any(|node| {
        matches!(node.kind(), SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR)
            && node
                .children()
                .next()
                .is_some_and(|class| text_of(&class).eq_ignore_ascii_case("parent"))
    })
}

/// Whether a class has what a member needs, through itself or what it extends.
fn lacks(cx: &Cx, class: &str, needs: &[(String, Need)]) -> Option<String> {
    let ty = Type::class(class.to_string());
    needs.iter().find_map(|(name, need)| {
        let present = match need {
            Need::Method => cx.index.find_method(&ty, name).is_some(),
            Need::Property => cx.index.find_property(&ty, name).is_some(),
            Need::Constant => cx.index.find_constant(&ty, name).is_some(),
        };
        (!present).then(|| name.clone())
    })
}

/// The text of a member as another file writes it: the names in full or imported, the visibility
/// widened when it was private, the indentation of the class it goes to.
fn written_member(
    rcx: &Rcx<'_>,
    member: &Member,
    from: &Loaded,
    to: &Loaded,
    to_class: &SyntaxNode,
    widen: bool,
    imports: &mut ClassWriter<'_>,
) -> String {
    let cx = &rcx.cx;
    let node = &member.node;
    let base = start(node);
    let mut text = text_slice(&from.text, node).to_string();
    let mut replacements: Vec<(u32, u32, String)> = Vec::new();
    if from.path != to.path {
        let resolver = php_index::extract::resolver_at(&from.root, start(&member.class));
        for name in node.descendants().filter(|name| name.kind() == NAME) {
            let Some(full) = fully_qualified(cx.index, &resolver, &name) else {
                continue;
            };
            let target = full.trim_start_matches('\\').to_string();
            let is_class = matches!(
                name.parent().map(|parent| parent.kind()),
                Some(
                    NEW_EXPR
                        | NAMED_TYPE
                        | ATTRIBUTE
                        | SCOPED_ACCESS_EXPR
                        | STATIC_PROPERTY_EXPR
                        | BINARY_EXPR
                        | CATCH_CLAUSE
                )
            );
            let written = if is_class {
                imports.written(&target)
            } else if target.contains('\\') {
                full
            } else {
                continue;
            };
            replacements.push((start(&name), end(&name), written));
        }
        for token in node
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
        {
            if token.kind() != DOC_COMMENT {
                continue;
            }
            let at = u32::from(token.text_range().start());
            for item in doc_items(token.text(), at) {
                if let DocItemKind::Class(written) = item.kind {
                    let lower = written.to_ascii_lowercase();
                    if matches!(lower.as_str(), "self" | "static" | "parent" | "$this")
                        || crate::inspections::util::is_builtin_type(&lower)
                    {
                        continue;
                    }
                    let target = resolver.resolve_class(&written);
                    replacements.push((item.start, item.end, imports.written(&target)));
                }
            }
        }
    }
    if widen {
        if let Some(token) = child_of(node, MODIFIER_LIST).and_then(|list| first_token(&list, PRIVATE_KW)) {
            replacements.push((
                u32::from(token.text_range().start()),
                u32::from(token.text_range().end()),
                "protected".to_string(),
            ));
        }
    }
    replacements.sort_by_key(|(from_at, _, _)| std::cmp::Reverse(*from_at));
    for (from_at, to_at, written) in replacements {
        text.replace_range((from_at - base) as usize..(to_at - base) as usize, &written);
    }
    let from_indent = indent_of(&from.text, start(node) as usize);
    let to_indent = with_cx(rcx, to, |to_cx| style_of(to_cx, to_class).indent);
    let eol = if to.text.contains("\r\n") { "\r\n" } else { "\n" };
    let protected: Vec<TextRange> = node
        .descendants()
        .filter(|inner| {
            matches!(inner.kind(), INTERPOLATED_STRING | HEREDOC | SHELL_EXEC_EXPR | LITERAL)
                && from.text[start(inner) as usize..end(inner) as usize].contains('\n')
        })
        .map(|inner| inner.text_range())
        .collect();
    let mut out = String::new();
    let mut offset = base as usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let inside = protected
            .iter()
            .any(|range| offset > usize::from(range.start()) && offset < usize::from(range.end()));
        offset += line.len();
        let body = line.trim_end_matches(['\r', '\n']);
        let newline = if line.ends_with('\n') { eol } else { "" };
        if index == 0 {
            out.push_str(&to_indent);
            out.push_str(body);
        } else if inside || body.trim().is_empty() {
            out.push_str(body);
        } else if let Some(rest) = body.strip_prefix(from_indent.as_str()) {
            out.push_str(&to_indent);
            out.push_str(rest);
        } else {
            out.push_str(body.trim_start_matches([' ', '\t']));
        }
        out.push_str(newline);
    }
    out
}

fn already_has(cx: &Cx, class: &str, member: &Member) -> bool {
    let ty = Type::class(class.to_string());
    if member.is_method {
        cx.index.find_method(&ty, &member.name).is_some()
    } else {
        cx.index.find_property(&ty, &member.name).is_some()
    }
}

fn pull_up(rcx: &Rcx<'_>, class_name: &str, target: &str, how: Pull) -> Result<Change, String> {
    let cx = &rcx.cx;
    let member = member_at(rcx).ok_or("There is no member here")?;
    let own = Loaded {
        path: rcx.renv.path.to_path_buf(),
        text: cx.text.to_string(),
        root: cx.root.clone(),
    };
    let (to, to_class) = declaration_in(rcx, target)?;
    if already_has(cx, target, &member) {
        return Err(format!("{} has a member of this name already", crate::short(target)));
    }
    let vis = visibility_of(&member.node);
    let target_kind = cx.index.class(target).map(|found| found.decl.kind);
    let mut draft = Draft::new(rcx.renv);
    match how {
        Pull::Move => {
            if uses_parent(&member.node) {
                return Err("The member calls parent, which would then mean another class".to_string());
            }
            if let Some(missing) = lacks(cx, target, &needs_of(&member.node)) {
                return Err(format!(
                    "The member needs '{missing}', which {} does not have",
                    crate::short(target)
                ));
            }
            let resolver = php_index::extract::resolver_at(&to.root, start(&to_class));
            let mut writer = ClassWriter::new(cx.index, &resolver, &to.text, &to.root, start(&to_class));
            let text = written_member(rcx, &member, &own, &to, &to_class, vis == "private", &mut writer);
            let group = if member.is_method {
                MemberGroup::Method
            } else {
                MemberGroup::Property
            };
            let edit = with_cx(rcx, &to, |to_cx| {
                member_insertion(to_cx, &to_class, text.trim_start(), group, false)
            });
            draft.edit(&to.path, edit);
            draft.edits(&to.path, writer.into_imports());
            draft.here(super::exprs::remove_member(cx.text, member.node.text_range()));
        }
        Pull::Declare => {
            if !member.is_method {
                return Err("Only a method can be declared above".to_string());
            }
            if vis != "public" && target_kind == Some(ClassKind::Interface) {
                return Err("An interface only holds public methods".to_string());
            }
            if vis == "private" {
                return Err("A private method cannot be declared above".to_string());
            }
            for below in cx.index.all_subtypes(target) {
                if below.decl.is_abstract
                    || below.decl.kind != ClassKind::Class
                    || below.decl.name.eq_ignore_ascii_case(class_name)
                {
                    continue;
                }
                if cx
                    .index
                    .find_method(&Type::class(below.decl.name.clone()), &member.name)
                    .is_none()
                {
                    return Err(format!(
                        "{} would be left without the method",
                        crate::short(&below.decl.name)
                    ));
                }
            }
            let signature = signature_of(
                rcx,
                &member,
                &own,
                &to,
                &to_class,
                target_kind == Some(ClassKind::Interface),
            )?;
            let edit = with_cx(rcx, &to, |to_cx| {
                member_insertion(to_cx, &to_class, signature.1.trim_start(), MemberGroup::Method, false)
            });
            draft.edit(&to.path, edit);
            draft.edits(&to.path, signature.0);
        }
    }
    draft.finish()
}

/// The declaration of a method without its body, for an interface or an abstract class.
fn signature_of(
    rcx: &Rcx<'_>,
    member: &Member,
    from: &Loaded,
    to: &Loaded,
    to_class: &SyntaxNode,
    interface: bool,
) -> Result<(Vec<TextEdit>, String), String> {
    let cx = &rcx.cx;
    let resolver = php_index::extract::resolver_at(&to.root, start(to_class));
    let mut writer = ClassWriter::new(cx.index, &resolver, &to.text, &to.root, start(to_class));
    let method = &member.node;
    let modifiers = child_of(method, MODIFIER_LIST);
    let is_static = modifiers.as_ref().is_some_and(|list| has_token(list, STATIC_KW));
    let vis = visibility_of(method);
    let name = child_of(method, NAME)
        .map(|name| text_of(&name))
        .ok_or("The method has no name")?;
    let params = child_of(method, PARAMETER_LIST).ok_or("The method has no parameters")?;
    let mut text = String::new();
    if !interface {
        text.push_str("abstract ");
    }
    text.push_str(vis);
    text.push(' ');
    if is_static {
        text.push_str("static ");
    }
    // The parameters and the return type are written as the declaring file has them, with the names
    // the other file reads in full or imported.
    let return_node = child_of(method, RETURN_TYPE).and_then(|node| node.children().next());
    let mut pieces: Vec<(u32, u32, String)> = Vec::new();
    if from.path != to.path {
        let from_resolver = php_index::extract::resolver_at(&from.root, start(&member.class));
        let named: Vec<SyntaxNode> = params
            .descendants()
            .chain(return_node.iter().flat_map(|node| node.descendants()))
            .filter(|node| node.kind() == NAME)
            .collect();
        for node in named {
            if let Some(full) = fully_qualified(cx.index, &from_resolver, &node) {
                let target = full.trim_start_matches('\\').to_string();
                let is_class = node.parent().is_some_and(|parent| parent.kind() == NAMED_TYPE);
                let written = if is_class {
                    writer.written(&target)
                } else if target.contains('\\') {
                    full
                } else {
                    continue;
                };
                pieces.push((start(&node), end(&node), written));
            }
        }
    }
    let mut parameter_text = text_slice(&from.text, &params).to_string();
    let mut return_text = return_node
        .as_ref()
        .map(|node| text_slice(&from.text, node).to_string());
    pieces.sort_by_key(|(from_at, _, _)| std::cmp::Reverse(*from_at));
    for (from_at, to_at, written) in pieces {
        let params_start = start(&params);
        if from_at >= params_start && to_at <= end(&params) {
            parameter_text.replace_range(
                (from_at - params_start) as usize..(to_at - params_start) as usize,
                &written,
            );
        } else if let (Some(node), Some(text)) = (&return_node, return_text.as_mut()) {
            let return_start = start(node);
            text.replace_range(
                (from_at - return_start) as usize..(to_at - return_start) as usize,
                &written,
            );
        }
    }
    let returned = return_text.map(|ty| format!(": {ty}")).unwrap_or_default();
    text.push_str(&format!("function {name}{parameter_text}{returned}"));
    text.push(';');
    let indent = with_cx(rcx, to, |to_cx| style_of(to_cx, to_class).indent);
    Ok((writer.into_imports(), format!("{indent}{text}")))
}

// Pushing down ------------------------------------------------------------------------------------

fn push_down(rcx: &Rcx<'_>, class_name: &str) -> Result<Change, String> {
    let cx = &rcx.cx;
    let member = member_at(rcx).ok_or("There is no member here")?;
    if visibility_of(&member.node) == "private" {
        return Err("A private member is only used in its own class".to_string());
    }
    if let Some(list) = child_of(&member.node, MODIFIER_LIST) {
        if has_token(&list, ABSTRACT_KW) {
            return Err("An abstract method is what the classes below have to implement".to_string());
        }
    }
    if uses_parent(&member.node) {
        return Err("The member calls parent, which would then mean another class".to_string());
    }
    let own = Loaded {
        path: rcx.renv.path.to_path_buf(),
        text: cx.text.to_string(),
        root: cx.root.clone(),
    };
    let mut targets: Vec<(Loaded, SyntaxNode, String)> = Vec::new();
    let mut skipped = 0;
    for below in cx.index.direct_subtypes(class_name) {
        let name = below.decl.name.clone();
        let probe = Member {
            node: member.node.clone(),
            name: member.name.clone(),
            is_method: member.is_method,
            class: member.class.clone(),
        };
        if below.decl.kind != ClassKind::Class {
            return Err(format!("{name} is not a class"));
        }
        let declares = if member.is_method {
            below.decl.method(&probe.name).is_some()
        } else {
            below.decl.property(&probe.name).is_some()
        };
        let (file, class) = declaration_in(rcx, &name)?;
        if declares {
            skipped += 1;
            continue;
        }
        targets.push((file, class, name));
    }
    if targets.is_empty() {
        return Err(if skipped > 0 {
            "Every class below has the member already".to_string()
        } else {
            "No class below takes the member".to_string()
        });
    }
    check_uses(rcx, &member, class_name, &own)?;
    let mut draft = Draft::new(rcx.renv);
    let group = if member.is_method {
        MemberGroup::Method
    } else {
        MemberGroup::Property
    };
    for (file, class, _) in &targets {
        let resolver = php_index::extract::resolver_at(&file.root, start(class));
        let mut writer = ClassWriter::new(cx.index, &resolver, &file.text, &file.root, start(class));
        let text = written_member(rcx, &member, &own, file, class, false, &mut writer);
        let edit = with_cx(rcx, file, |file_cx| {
            member_insertion(file_cx, class, text.trim_start(), group, false)
        });
        draft.edit(&file.path, edit);
        draft.edits(&file.path, writer.into_imports());
    }
    draft.here(super::exprs::remove_member(cx.text, member.node.text_range()));
    draft.finish()
}

/// Every use of a member has to be inside the member itself or in a class below, through `$this`.
fn check_uses(rcx: &Rcx<'_>, member: &Member, class_name: &str, own: &Loaded) -> Result<(), String> {
    let cx = &rcx.cx;
    let symbol = if member.is_method {
        Symbol::Method {
            class: class_name.to_string(),
            name: member.name.clone(),
        }
    } else {
        Symbol::Property {
            class: class_name.to_string(),
            name: member.name.clone(),
        }
    };
    let current = Current {
        path: rcx.renv.path,
        text: cx.text,
        root: &cx.root,
    };
    for file_hits in hits_of_symbols(cx.index, rcx.renv.sources, &current, std::slice::from_ref(&symbol)) {
        let root = if file_hits.path == rcx.renv.path {
            own.root.clone()
        } else {
            let text = rcx.renv.sources.text(&file_hits.path).ok_or("A file cannot be read")?;
            parse(&text).syntax()
        };
        for hit in &file_hits.hits {
            if hit.kind == crate::refs::HitKind::Declaration {
                continue;
            }
            let token = root.covering_element(hit.range).into_token();
            let Some(name) = token.and_then(|token| token.parent()) else {
                continue;
            };
            if file_hits.path == rcx.renv.path && member.node.text_range().contains_range(hit.range) {
                continue;
            }
            let class = name.ancestors().find(|node| ast::is_class_like(node.kind()));
            let inside_below = class.as_ref().is_some_and(|class| {
                let resolver = php_index::extract::resolver_at(&root, start(class));
                child_of(class, NAME).is_some_and(|name| {
                    let fqn = resolver.qualify(&text_of(&name));
                    cx.index.is_subclass_of(&fqn, class_name) && !fqn.eq_ignore_ascii_case(class_name)
                })
            });
            let receiver = name
                .parent()
                .and_then(|access| access.children().next())
                .map(|node| text_of(&node).to_ascii_lowercase());
            let own_receiver = matches!(receiver.as_deref(), Some("$this" | "self" | "static"));
            if !inside_below || !own_receiver {
                return Err(format!(
                    "The member is used in {}, which would lose it",
                    file_hits.path.display()
                ));
            }
        }
    }
    Ok(())
}
