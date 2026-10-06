use php_syntax::{LevelSeverity, Parse, PhpVersion, TextRange, check_language_level};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A problem in a file, from the syntax or from the language level it is read at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: TextRange,
    pub message: String,
    pub severity: DiagnosticSeverity,
    /// Set for deprecated syntax, which a client may draw struck through.
    pub deprecated: bool,
    /// Set for code that does nothing, which a client may draw faded.
    pub unnecessary: bool,
    /// `syntax` for a parse error, the feature id for a language level finding.
    pub code: &'static str,
}

/// The syntax errors of a parse, and what the language level does not allow, in source order.
pub fn diagnostics(parse: &Parse, level: PhpVersion) -> Vec<Diagnostic> {
    let mut found: Vec<Diagnostic> = parse
        .errors()
        .iter()
        .map(|error| Diagnostic {
            range: error.range,
            message: error.message.clone(),
            severity: DiagnosticSeverity::Error,
            deprecated: false,
            unnecessary: false,
            code: "syntax",
        })
        .collect();
    found.extend(
        check_language_level(&parse.syntax(), level)
            .into_iter()
            .map(|finding| Diagnostic {
                range: finding.range,
                message: finding.message,
                severity: match finding.severity {
                    LevelSeverity::Error => DiagnosticSeverity::Error,
                    LevelSeverity::Warning => DiagnosticSeverity::Warning,
                },
                deprecated: finding.deprecated,
                unnecessary: false,
                code: finding.feature,
            }),
    );
    found.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use php_syntax::parse;

    #[test]
    fn merges_syntax_errors_and_level_findings_in_order() {
        let parsed = parse("<?php\n$a = $b |> f(...);\n$c = ;\n");
        let found = diagnostics(&parsed, PhpVersion::V8_4);
        let codes: Vec<_> = found.iter().map(|found| found.code).collect();
        assert_eq!(codes, ["pipe-operator", "syntax"]);
        assert!(
            diagnostics(&parsed, PhpVersion::DEFAULT)
                .iter()
                .all(|found| found.code == "syntax")
        );
    }
}
