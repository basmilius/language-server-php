use std::path::PathBuf;

use php_analysis::DiagnosticSeverity;
use php_analysis::inspections::{InspectionSettings, Override};
use php_format::{BraceStyle, FormatOptions};
use php_syntax::PhpVersion;
use serde_json::Value;

/// The section a client answers `workspace/configuration` for, and pushes in `didChangeConfiguration`.
pub const SECTION: &str = "phpLanguageServer";

/// The settings the server reads. Both `{ "phpVersion": "8.4" }` and the same object under
/// [`SECTION`] are understood, so a client may pass the settings as it likes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Settings {
    /// The language level for a project whose `composer.json` does not say.
    pub php_version: Option<PhpVersion>,
    /// A folder the server may keep its cache and the standard library stubs in. Without one it keeps
    /// nothing between runs and does not fetch the stubs.
    pub storage_path: Option<PathBuf>,
    /// A folder of phpstorm-stubs to read instead of fetching them into the storage path.
    pub stubs_path: Option<PathBuf>,
    /// `inlayHints.parameterNames`: show the name of the parameter in front of an argument.
    pub hint_parameter_names: Option<bool>,
    /// `inlayHints.closureTypes`: show the type a function promises for a closure's parameter.
    pub hint_closure_types: Option<bool>,
    /// `inlayHints.propertyTypes`: show the type arguments a framework gives a property.
    pub hint_property_types: Option<bool>,
    /// `inspections`: a switch or a severity per inspection code. `None` when the key is absent.
    pub inspections: Option<InspectionSettings>,
    /// `format`: what the formatter is told to do besides the indentation the client sends.
    pub format: Option<FormatSettings>,
    /// `usages.packages`: find usages and incoming calls also read the installed packages.
    pub usages_packages: Option<bool>,
    /// `sql`: how SQL is found in strings and read, as given; `sql::config` reads it per document.
    pub sql: Option<Value>,
}

/// The keys of `format`, each of which leaves the formatter's default alone when absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatSettings {
    pub class_brace: Option<BraceStyle>,
    pub function_brace: Option<BraceStyle>,
    pub blank_lines_between_members: Option<usize>,
    pub align_assignments: Option<bool>,
    pub align_array_arrows: Option<bool>,
    pub line_length: Option<usize>,
    /// Whether the `.editorconfig` of a project sets the indent and the line length.
    pub editorconfig: Option<bool>,
}

impl FormatSettings {
    fn from_value(value: &Value) -> FormatSettings {
        let brace = |key: &str| match value.get(key).and_then(Value::as_str) {
            Some("nextLine") => Some(BraceStyle::NextLine),
            Some("sameLine") => Some(BraceStyle::SameLine),
            _ => None,
        };
        let number = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_u64)
                .and_then(|number| usize::try_from(number).ok())
        };
        let flag = |key: &str| value.get(key).and_then(Value::as_bool);
        FormatSettings {
            class_brace: brace("classBrace"),
            function_brace: brace("functionBrace"),
            blank_lines_between_members: number("blankLinesBetweenMembers"),
            align_assignments: flag("alignAssignments"),
            align_array_arrows: flag("alignArrayArrows"),
            line_length: number("lineLength"),
            editorconfig: flag("editorconfig"),
        }
    }

    /// The options with the keys that are set laid over them.
    pub fn apply(&self, mut options: FormatOptions) -> FormatOptions {
        options.class_brace = self.class_brace.unwrap_or(options.class_brace);
        options.function_brace = self.function_brace.unwrap_or(options.function_brace);
        options.blank_lines_between_members = self
            .blank_lines_between_members
            .unwrap_or(options.blank_lines_between_members);
        options.align_assignments = self.align_assignments.unwrap_or(options.align_assignments);
        options.align_array_arrows = self.align_array_arrows.unwrap_or(options.align_array_arrows);
        options.line_length = self.line_length.unwrap_or(options.line_length);
        options
    }
}

impl Settings {
    pub fn from_value(value: &Value) -> Settings {
        let object = value.get(SECTION).unwrap_or(value);
        let text = |key: &str| object.get(key).and_then(Value::as_str);
        let hints = object.get("inlayHints");
        let flag = |key: &str| hints.and_then(|hints| hints.get(key)).and_then(Value::as_bool);
        Settings {
            php_version: text("phpVersion").and_then(PhpVersion::parse),
            storage_path: text("storagePath").map(PathBuf::from),
            stubs_path: text("stubsPath").map(PathBuf::from),
            hint_parameter_names: flag("parameterNames"),
            hint_closure_types: flag("closureTypes"),
            hint_property_types: flag("propertyTypes"),
            inspections: object.get("inspections").map(parse_inspections),
            format: object
                .get("format")
                .filter(|value| value.is_object())
                .map(FormatSettings::from_value),
            usages_packages: object
                .get("usages")
                .and_then(|usages| usages.get("packages"))
                .and_then(Value::as_bool),
            sql: object.get("sql").filter(|sql| sql.is_object()).cloned(),
        }
    }
}

/// `{ "unused-import": "off", "undefined-class": { "severity": "warning" }, "deprecated": false }`.
fn parse_inspections(value: &Value) -> InspectionSettings {
    let mut settings = InspectionSettings::default();
    let Some(map) = value.as_object() else {
        return settings;
    };
    for (code, choice) in map {
        let choice = match choice {
            Value::Bool(enabled) => Override {
                enabled: Some(*enabled),
                severity: None,
            },
            Value::String(text) => override_of(text),
            Value::Object(fields) => {
                let named = fields.get("severity").and_then(Value::as_str).map(override_of);
                Override {
                    enabled: fields
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .or_else(|| named.and_then(|named| named.enabled)),
                    severity: named.and_then(|named| named.severity),
                }
            }
            _ => continue,
        };
        settings.set(code, choice);
    }
    settings
}

fn override_of(text: &str) -> Override {
    let severity = match text.to_ascii_lowercase().as_str() {
        "off" | "none" | "false" => {
            return Override {
                enabled: Some(false),
                severity: None,
            };
        }
        "error" => DiagnosticSeverity::Error,
        "warning" | "warn" => DiagnosticSeverity::Warning,
        "information" | "info" => DiagnosticSeverity::Information,
        "hint" => DiagnosticSeverity::Hint,
        _ => return Override::default(),
    };
    Override {
        enabled: Some(true),
        severity: Some(severity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_version_from_either_shape() {
        let direct = Settings::from_value(&json!({ "phpVersion": "8.3" }));
        assert_eq!(direct.php_version, Some(PhpVersion::V8_3));
        let nested = Settings::from_value(&json!({ "phpLanguageServer": { "phpVersion": "^8.1" } }));
        assert_eq!(nested.php_version, Some(PhpVersion::V8_1));
        assert_eq!(Settings::from_value(&json!(null)).php_version, None);
        assert_eq!(
            Settings::from_value(&json!({ "phpVersion": "nonsense" })).php_version,
            None
        );
    }

    #[test]
    fn reads_the_format_keys_and_lays_them_over_the_defaults() {
        let settings = Settings::from_value(&json!({
            "format": { "classBrace": "sameLine", "lineLength": 100, "alignArrayArrows": true, "functionBrace": "nonsense" }
        }));
        let options = settings
            .format
            .expect("the key is there")
            .apply(FormatOptions::default());
        assert_eq!(options.class_brace, BraceStyle::SameLine);
        assert_eq!(options.function_brace, BraceStyle::NextLine);
        assert_eq!(options.line_length, 100);
        assert!(options.align_array_arrows);
        assert!(!options.align_assignments);
        assert_eq!(Settings::from_value(&json!({})).format, None);
    }

    #[test]
    fn reads_the_storage_and_stub_folders() {
        let settings = Settings::from_value(&json!({ "storagePath": "/cache", "stubsPath": "/stubs" }));
        assert_eq!(settings.storage_path, Some(PathBuf::from("/cache")));
        assert_eq!(settings.stubs_path, Some(PathBuf::from("/stubs")));
    }

    #[test]
    fn reads_inspection_switches_and_severities() {
        let settings = Settings::from_value(&json!({
            "inspections": {
                "unused-import": "off",
                "undefined-class": "hint",
                "deprecated": { "severity": "error" },
                "unused-variable": { "enabled": false },
                "missing-strict-types": true,
            }
        }));
        let inspections = settings.inspections.expect("the key is there");
        let severity =
            |code: &str| inspections.severity_of(php_analysis::inspections::inspection_info(code).expect("a code"));
        assert_eq!(severity("unused-import"), None);
        assert_eq!(severity("undefined-class"), Some(DiagnosticSeverity::Hint));
        assert_eq!(severity("deprecated"), Some(DiagnosticSeverity::Error));
        assert_eq!(severity("unused-variable"), None);
        assert_eq!(severity("missing-strict-types"), Some(DiagnosticSeverity::Hint));
        assert_eq!(severity("undefined-function"), Some(DiagnosticSeverity::Error));
        assert!(Settings::from_value(&json!({})).inspections.is_none());
    }

    #[test]
    fn reads_the_inlay_hint_switches() {
        let settings = Settings::from_value(&json!({ "inlayHints": { "parameterNames": false } }));
        assert_eq!(settings.hint_parameter_names, Some(false));
        assert_eq!(settings.hint_closure_types, None);
    }
}
