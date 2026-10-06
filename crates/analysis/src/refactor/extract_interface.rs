//! Extract interface: a new interface next to a class, with the class's public methods as its
//! signatures, which the class then implements. The interface takes `{Class}Interface` as its name,
//! which the client is asked to rename right away.

use php_index::{Origin, UseKind};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode, SyntaxToken};

use super::draft::{Draft, focus};
use super::{Rcx, Refactor, RefactorKind};
use crate::ast::{child_of, end, has_token, start, text_of};
use crate::completion::TextEdit;

/// A public method of the class, as the interface declares it.
struct Signature {
    doc: Option<String>,
    is_static: bool,
    /// From `function` to the end of the return type.
    text: String,
}

pub(super) fn offer<'a>(rcx: &'a Rcx<'a>, out: &mut Vec<Refactor<'a>>) {
    let cx = &rcx.cx;
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
        .filter(|declaration| declaration.kind() == CLASS_DECLARATION)
    else {
        return;
    };
    let Some(name) = child_of(&class, NAME) else {
        return;
    };
    let analyzer = cx.file.analyzer(&class);
    let qualified = analyzer.resolver.qualify(&text_of(&name));
    if cx
        .index
        .class(&qualified)
        .is_none_or(|found| found.file.origin != Origin::Project)
    {
        return;
    }
    if public_methods(&class).is_empty() {
        return;
    }
    let short = format!("{}Interface", text_of(&name));
    let title = format!("Extract interface {short}");
    out.push(Refactor::new(
        format!("extract-interface:{}", start(&class)),
        title,
        RefactorKind::Extract,
        false,
        move || extract(rcx, &class, &short),
    ));
}

fn extract(rcx: &Rcx<'_>, class: &SyntaxNode, short: &str) -> Result<super::Change, String> {
    let cx = &rcx.cx;
    let analyzer = cx.file.analyzer(class);
    let namespace = analyzer.resolver.namespace.clone();
    let interface = if namespace.is_empty() {
        short.to_string()
    } else {
        format!("{namespace}\\{short}")
    };
    let path = rcx
        .renv
        .path
        .parent()
        .map(|dir| dir.join(format!("{short}.php")))
        .ok_or("The class has no folder")?;
    if cx.index.class(&interface).is_some() || path.exists() || rcx.renv.sources.text(&path).is_some() {
        return Err(format!("'{interface}' is taken"));
    }
    if analyzer
        .resolver
        .imports(UseKind::Class)
        .any(|(alias, _)| alias.eq_ignore_ascii_case(short))
    {
        return Err(format!("The file imports another '{short}'"));
    }
    let signatures = public_methods(class)
        .iter()
        .map(signature_of)
        .collect::<Result<Vec<_>, String>>()?;
    let indent = rcx.renv.format.indent.unit();
    let mut body = String::new();
    for (position, signature) in signatures.iter().enumerate() {
        if position > 0 {
            body.push('\n');
        }
        if let Some(doc) = &signature.doc {
            body.push_str(&indent);
            body.push_str(&reindent(doc, &indent));
            body.push('\n');
        }
        body.push_str(&indent);
        body.push_str(if signature.is_static {
            "public static "
        } else {
            "public "
        });
        body.push_str(&signature.text);
        body.push_str(";\n");
    }
    let mut text = String::from("<?php\n\n");
    if cx.text.contains("declare(strict_types=1)") {
        text.push_str("declare(strict_types=1);\n\n");
    }
    if !namespace.is_empty() {
        text.push_str(&format!("namespace {namespace};\n\n"));
    }
    let imports = imports_for(&analyzer.resolver, &body);
    if !imports.is_empty() {
        text.push_str(&imports.join("\n"));
        text.push_str("\n\n");
    }
    text.push_str(&format!("interface {short}\n{{\n{body}}}\n"));
    let mut draft = Draft::new(rcx.renv);
    draft.here(implements_edit(class, short));
    draft.create_file(path, text);
    draft.finish()
}

/// The public methods of a class that an interface can declare: not the constructor, nor another
/// magic method.
fn public_methods(class: &SyntaxNode) -> Vec<SyntaxNode> {
    let Some(body) = child_of(class, CLASS_BODY) else {
        return Vec::new();
    };
    body.children()
        .filter(|member| member.kind() == METHOD_DECLARATION)
        .filter(|method| {
            let modifiers = child_of(method, MODIFIER_LIST);
            let hidden = modifiers
                .as_ref()
                .is_some_and(|list| has_token(list, PRIVATE_KW) || has_token(list, PROTECTED_KW));
            let magic = child_of(method, NAME).is_some_and(|name| text_of(&name).starts_with("__"));
            !hidden && !magic
        })
        .collect()
}

fn signature_of(method: &SyntaxNode) -> Result<Signature, String> {
    let function = method
        .children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|token| token.kind() == FUNCTION_KW)
        .ok_or("A method has no `function`")?;
    let last = child_of(method, RETURN_TYPE)
        .or_else(|| child_of(method, PARAMETER_LIST))
        .ok_or("A method has no parameters")?;
    let from = u32::from(function.text_range().start());
    let to = end(&last);
    let text = method.text().slice(
        php_syntax::TextSize::from(from) - method.text_range().start()
            ..php_syntax::TextSize::from(to) - method.text_range().start(),
    );
    let text = text.to_string();
    if let Some(parameters) = child_of(method, PARAMETER_LIST) {
        // `self` in an interface is the interface, which a parameter of the class cannot narrow to.
        let names_self = parameters
            .descendants()
            .filter(|node| node.kind() == NAME)
            .any(|name| matches!(text_of(&name).to_ascii_lowercase().as_str(), "self" | "static"));
        if names_self {
            let name = child_of(method, NAME).map(|name| text_of(&name)).unwrap_or_default();
            return Err(format!("A parameter of {name}() names the class itself"));
        }
    }
    let modifiers = child_of(method, MODIFIER_LIST);
    Ok(Signature {
        doc: leading_doc(method).map(|token| token.text().to_string()),
        is_static: modifiers.as_ref().is_some_and(|list| has_token(list, STATIC_KW)),
        text,
    })
}

fn leading_doc(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut found = None;
    for element in node.children_with_tokens() {
        match element {
            SyntaxElement::Token(token) if token.kind() == DOC_COMMENT => found = Some(token),
            SyntaxElement::Token(token) if token.kind().is_trivia() => {}
            _ => break,
        }
    }
    found
}

/// A doc comment's lines after the first, aligned to the indentation of the interface.
fn reindent(doc: &str, indent: &str) -> String {
    let mut lines = doc.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    for line in lines {
        out.push('\n');
        out.push_str(indent);
        out.push(' ');
        out.push_str(line.trim_start());
    }
    out
}

/// The `use` lines of the class's file that the signatures need.
fn imports_for(resolver: &php_index::NameResolver, body: &str) -> Vec<String> {
    let words: std::collections::HashSet<String> = body
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut out = Vec::new();
    for (kind, keyword) in [
        (UseKind::Class, "use"),
        (UseKind::Function, "use function"),
        (UseKind::Constant, "use const"),
    ] {
        let mut lines: Vec<String> = resolver
            .imports(kind)
            .filter(|(alias, _)| words.contains(&alias.to_ascii_lowercase()))
            .map(|(alias, target)| {
                let last = target.rsplit('\\').next().unwrap_or(target);
                if last.eq_ignore_ascii_case(alias) {
                    format!("{keyword} {target};")
                } else {
                    format!("{keyword} {target} as {alias};")
                }
            })
            .collect();
        lines.sort();
        out.extend(lines);
    }
    out
}

/// `implements Name` on the class, or `, Name` after what it implements already.
fn implements_edit(class: &SyntaxNode, short: &str) -> TextEdit {
    let named = focus(short);
    if let Some(clause) = child_of(class, IMPLEMENTS_CLAUSE) {
        let at = end(&clause);
        return TextEdit {
            start: at,
            end: at,
            new_text: format!(", {named}"),
        };
    }
    let after = child_of(class, EXTENDS_CLAUSE)
        .or_else(|| child_of(class, NAME))
        .map_or_else(|| start(class), |node| end(&node));
    TextEdit {
        start: after,
        end: after,
        new_text: format!(" implements {named}"),
    }
}
