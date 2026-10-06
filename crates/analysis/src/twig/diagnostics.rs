//! What is certainly wrong in a Twig template: a delimiter that is not closed, a token no tag or
//! expression takes, an expression or a name that is missing, a block tag that is never closed or
//! closes nothing, a template or a route the project does not have, and a function, filter or test
//! no extension declares.

use std::path::Path;

use php_index::Index;
use php_index::framework::keys::{KeyKind, is_missing};
use php_index::framework::twig::{TwigExtensions, TwigKind};
use php_syntax::TextRange;

use super::parse::Expr;
use super::{Item, TagBody, Template, tag_exprs};
use crate::ast::range_of;
use crate::diagnostics::{Diagnostic, DiagnosticSeverity};
use crate::inspections::{INSPECTIONS, InspectionSettings};

/// The tags that open a block, with the tag that closes it.
const BLOCKS: &[(&str, &str)] = &[
    ("if", "endif"),
    ("for", "endfor"),
    ("block", "endblock"),
    ("macro", "endmacro"),
    ("set", "endset"),
    ("with", "endwith"),
    ("apply", "endapply"),
    ("autoescape", "endautoescape"),
    ("embed", "endembed"),
    ("sandbox", "endsandbox"),
    ("spaceless", "endspaceless"),
    ("filter", "endfilter"),
    ("cache", "endcache"),
    ("guard", "endguard"),
    ("trans", "endtrans"),
    ("stopwatch", "endstopwatch"),
    ("verbatim", "endverbatim"),
    ("raw", "endraw"),
];

/// The functions Twig's parser reads itself in versions that do not declare them.
const PARSED: &[&str] = &["parent", "block", "attribute"];

pub fn diagnostics(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    settings: &InspectionSettings,
    ready: bool,
) -> Vec<Diagnostic> {
    let template = Template::read(index, path, text);
    let mut out: Vec<Diagnostic> = template
        .errors
        .iter()
        .map(|(start, end, message)| {
            diagnostic(
                range_of(*start, *end),
                message.clone(),
                DiagnosticSeverity::Error,
                "syntax",
            )
        })
        .collect();
    // What an unclosed delimiter holds runs into the markup after it; the delimiter is the error.
    let unclosed: Vec<u32> = template
        .errors
        .iter()
        .filter(|(_, _, message)| message.ends_with("is not closed"))
        .map(|(start, ..)| *start)
        .collect();
    for item in &template.items {
        match item {
            Item::Output { start, .. } if unclosed.contains(start) => {}
            Item::Tag(tag) if unclosed.contains(&tag.start) => {}
            Item::Output { expr, .. } => missing(expr, &mut out),
            Item::Tag(tag) if tag.name != "apply" => {
                for expr in tag_exprs(&tag.body) {
                    missing(expr, &mut out);
                }
            }
            Item::Tag(_) => {}
        }
    }
    if let Some(severity) = severity(settings, "unbalanced-directive") {
        balance(text, &template, severity, &mut out);
    }
    if ready {
        unknown(index, &template, settings, &mut out);
    }
    out.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
    out
}

fn diagnostic(range: TextRange, message: String, severity: DiagnosticSeverity, code: &'static str) -> Diagnostic {
    Diagnostic {
        range,
        message,
        severity,
        deprecated: false,
        unnecessary: false,
        code,
    }
}

fn severity(settings: &InspectionSettings, code: &str) -> Option<DiagnosticSeverity> {
    INSPECTIONS
        .iter()
        .find(|info| info.code == code)
        .and_then(|info| settings.severity_of(info))
}

/// The holes of an expression: an operand, an attribute, a filter or a test that is not written.
fn missing(expr: &Expr, out: &mut Vec<Diagnostic>) {
    expr.walk(&mut |inner| {
        let found = match inner {
            Expr::Missing(at) => Some((*at, "An expression is expected")),
            Expr::Attribute { name, start, .. } if name.is_empty() => Some((*start, "An attribute name is expected")),
            Expr::Filter { name, start, .. } if name.is_empty() => Some((*start, "A filter name is expected")),
            Expr::Test { name, start, .. } if name.is_empty() => Some((*start, "A test name is expected")),
            _ => None,
        };
        if let Some((at, message)) = found {
            out.push(diagnostic(
                range_of(at, at),
                message.to_string(),
                DiagnosticSeverity::Error,
                "syntax",
            ));
        }
    });
}

/// Block tags that are never closed and end tags that close nothing.
fn balance(text: &str, template: &Template, severity: DiagnosticSeverity, out: &mut Vec<Diagnostic>) {
    let ends: std::collections::HashSet<&str> = template
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Tag(tag) => tag.name.strip_prefix("end"),
            _ => None,
        })
        .collect();
    let closer_of = |tag: &super::Tag| -> Option<String> {
        if let Some((_, closer)) = BLOCKS.iter().find(|(opener, _)| *opener == tag.name) {
            let opens = match &tag.body {
                TagBody::Block { short, .. } => short.is_none(),
                TagBody::Set { values, .. } => values.is_empty(),
                _ => true,
            };
            return opens.then(|| closer.to_string());
        }
        // A tag of an extension that the template closes with `end<name>` is a block.
        (!tag.name.starts_with("end") && ends.contains(tag.name.as_str())).then(|| format!("end{}", tag.name))
    };
    let mut stack: Vec<(&super::Tag, String)> = Vec::new();
    for item in &template.items {
        let Item::Tag(tag) = item else {
            continue;
        };
        if let Some(closer) = closer_of(tag) {
            stack.push((tag, closer));
            continue;
        }
        let continues = match tag.name.as_str() {
            "else" => Some(&["if", "for"][..]),
            "elseif" => Some(&["if"][..]),
            _ => None,
        };
        if let Some(openers) = continues {
            if !stack
                .last()
                .is_some_and(|(open, _)| openers.contains(&open.name.as_str()))
            {
                out.push(diagnostic(
                    range_of(tag.start, tag.end),
                    format!("'{{% {} %}}' is not inside a block it belongs to", tag.name),
                    severity,
                    "unbalanced-directive",
                ));
            }
            continue;
        }
        if tag.name.starts_with("end")
            && (BLOCKS.iter().any(|(_, closer)| *closer == tag.name) || ends.contains(&tag.name[3..]))
        {
            if stack.last().is_some_and(|(_, closer)| *closer == tag.name) {
                stack.pop();
            } else {
                out.push(diagnostic(
                    range_of(tag.start, tag.end),
                    format!("'{{% {} %}}' closes nothing", tag.name),
                    severity,
                    "unbalanced-directive",
                ));
            }
        }
    }
    for (tag, _) in stack {
        let written = &text[tag.start as usize..tag.end as usize];
        out.push(diagnostic(
            range_of(tag.start, tag.end),
            format!("'{written}' is never closed"),
            severity,
            "unbalanced-directive",
        ));
    }
}

/// The templates and routes the project does not have, and the functions, filters and tests no
/// extension declares.
fn unknown(index: &Index, template: &Template, settings: &InspectionSettings, out: &mut Vec<Diagnostic>) {
    for name in &template.names {
        let Some(code) = name.kind.inspection() else {
            continue;
        };
        let Some(severity) = severity(settings, code) else {
            continue;
        };
        if name.value.is_empty() || !is_missing(index, name.kind, &name.value) {
            continue;
        }
        let message = match name.kind {
            KeyKind::Template => format!("The template '{}' does not exist", name.value),
            KeyKind::Route => format!("No route is named '{}'", name.value),
            _ => format!("The translation '{}' is not in the language files", name.value),
        };
        out.push(diagnostic(range_of(name.start, name.end), message, severity, code));
    }
    let extensions = index.section::<TwigExtensions>();
    // Without Twig's own extension the packages were not read, and nothing is certain.
    if extensions.find(TwigKind::Filter, "upper").is_none() {
        return;
    }
    let macros: Vec<&str> = template
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Tag(tag) => match &tag.body {
                TagBody::Import { names, .. } => Some(names.iter().map(|(name, ..)| name.as_str()).collect::<Vec<_>>()),
                TagBody::Macro { name, .. } => Some(vec![name.0.as_str()]),
                _ => None,
            },
            _ => None,
        })
        .flatten()
        .collect();
    for call in &template.calls {
        if call.name.is_empty() || extensions.find(call.kind, &call.name).is_some() {
            continue;
        }
        let (code, what) = match call.kind {
            TwigKind::Function => {
                if PARSED.contains(&call.name.as_str()) || macros.contains(&call.name.as_str()) {
                    continue;
                }
                ("unknown-twig-function", "function")
            }
            TwigKind::Filter => ("unknown-twig-filter", "filter"),
            TwigKind::Test => ("unknown-twig-test", "test"),
        };
        let Some(severity) = severity(settings, code) else {
            continue;
        };
        out.push(diagnostic(
            range_of(call.start, call.end),
            format!("No Twig extension declares the {what} '{}'", call.name),
            severity,
            code,
        ));
    }
}
