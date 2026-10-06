//! What is certainly wrong in a template: PHP that does not parse in an echo, a `@php` block or the
//! arguments of a directive, a block directive that is not closed or closes nothing, and the views,
//! routes, config keys and translations a template names that the project does not have, under the
//! same conditions as in PHP.

use std::path::Path;

use php_index::framework::keys::{KeyKind, is_missing};
use php_index::{Index, Type};
use php_syntax::{TextRange, parse};

use super::{Imbalance, Template};
use crate::ast::range_of;
use crate::context::FileContext;
use crate::diagnostics::{Diagnostic, DiagnosticSeverity};
use crate::frameworks::keys::keys_in;
use crate::inspections::{INSPECTIONS, InspectionSettings};

/// The directives that include a view only when it exists.
const GUARDED: &[&str] = &["includeif", "includefirst"];

pub fn diagnostics(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    settings: &InspectionSettings,
    ready: bool,
) -> Vec<Diagnostic> {
    let template = Template::read(index, path, text, given);
    let mut out = Vec::new();
    let parsed = parse(&template.virt.text);
    for error in parsed.errors() {
        let range = template.virt.range_to_source(error.range).or_else(|| {
            let start = template.virt.to_source(u32::from(error.range.start()))?;
            Some(range_of(start, start))
        });
        if let Some(range) = range {
            out.push(diagnostic(
                range,
                error.message.clone(),
                DiagnosticSeverity::Error,
                "syntax",
            ));
        }
    }
    if let Some(severity) = severity(settings, "unbalanced-directive") {
        for imbalance in &template.imbalances {
            let (message, at, end) = match imbalance {
                Imbalance::Unclosed { name, at, end } => (format!("'@{name}' is never closed"), *at, *end),
                Imbalance::Unopened { name, at, end } => (format!("'@{name}' has no block to belong to"), *at, *end),
            };
            out.push(diagnostic(range_of(at, end), message, severity, "unbalanced-directive"));
        }
    }
    if ready && index.frameworks().laravel {
        unknown_names(index, &template, settings, &mut out);
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

fn message(kind: KeyKind, name: &str) -> String {
    match kind {
        KeyKind::Config => format!("The config key '{name}' is not in the config files"),
        KeyKind::Route => format!("No route is named '{name}'"),
        KeyKind::View => format!("The view '{name}' does not exist"),
        _ => format!("The translation '{name}' is not in the language files"),
    }
}

/// The names of the directives and of the PHP that the project does not declare.
fn unknown_names(index: &Index, template: &Template, settings: &InspectionSettings, out: &mut Vec<Diagnostic>) {
    let mut report = |kind: KeyKind, name: &str, range: TextRange| {
        let Some(code) = kind.inspection() else {
            return;
        };
        let Some(severity) = severity(settings, code) else {
            return;
        };
        if name.is_empty() || !is_missing(index, kind, name) {
            return;
        }
        out.push(diagnostic(range, message(kind, name), severity, code));
    };
    for name in &template.names {
        let guarded = template.nodes.iter().any(|node| match node {
            super::scan::Node::Directive(directive) => {
                GUARDED.contains(&directive.name.to_ascii_lowercase().as_str())
                    && directive
                        .args
                        .is_some_and(|(start, end)| start <= name.start && name.end <= end)
            }
            _ => false,
        });
        if !guarded {
            report(name.kind, &name.value, range_of(name.start, name.end));
        }
    }
    let root = template.root();
    let ctx = FileContext::new(index, &root);
    for key in keys_in(&ctx) {
        if key.guarded {
            continue;
        }
        if let Some(range) = template.virt.range_to_source(key.range) {
            report(key.kind, &key.value, range);
        }
    }
}
