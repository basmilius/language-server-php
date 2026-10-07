//! The `sql` setting: whether and how SQL is found in strings, and what it is read as, at the top
//! level and per path in `overrides`.

use std::path::{Path, PathBuf};

use php_analysis::sql::Detection;
use serde_json::{Map, Value};

/// The keys an entry of `overrides` may set.
const PER_PATH: [&str; 4] = ["dialect", "version", "schema", "sqlMode"];

/// What the `sql` setting says for one document.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub enabled: bool,
    pub detection: Detection,
    pub settings: sql_embed::Settings,
    /// The settings name a dialect for the document, which goes before what the code suggests.
    pub dialect_set: bool,
    /// The snapshot, as a path on disk.
    pub schema: Option<PathBuf>,
    /// What could not be read.
    pub problems: Vec<String>,
}

/// A path of the settings: absolute, a `file:` URI, or relative to `base`.
pub fn resolve_path(text: &str, base: Option<&Path>) -> Option<PathBuf> {
    if text.starts_with("file:") {
        let uri: lsp_types::Uri = text.parse().ok()?;
        return lsc_server::paths::uri_to_path(&uri);
    }
    let path = PathBuf::from(text);
    if path.is_absolute() {
        return Some(path);
    }
    Some(base?.join(path))
}

fn detection(value: Option<&Value>) -> Detection {
    let mut detection = Detection::default();
    let Some(value) = value else {
        return detection;
    };
    let flag = |key: &str, default: bool| value.get(key).and_then(Value::as_bool).unwrap_or(default);
    detection.markers = flag("markers", detection.markers);
    detection.sinks = flag("sinks", detection.sinks);
    detection.heuristic = flag("heuristic", detection.heuristic);
    if let Some(threshold) = value.get("threshold").and_then(Value::as_f64) {
        detection.threshold = (threshold as f32).clamp(0.0, 1.0);
    }
    detection
}

/// The `sql` setting for a document at `path`, with relative paths read from `base`.
pub fn resolve(value: Option<&Value>, path: Option<&Path>, base: Option<&Path>) -> Resolved {
    let empty = Map::new();
    let object = value.and_then(Value::as_object).unwrap_or(&empty);
    let mut layers: Vec<(usize, &Map<String, Value>)> = Vec::new();
    if let (Some(path), Some(overrides)) = (path, object.get("overrides").and_then(Value::as_array)) {
        for entry in overrides.iter().filter_map(Value::as_object) {
            let Some(at) = entry
                .get("path")
                .and_then(Value::as_str)
                .and_then(|text| resolve_path(text, base))
            else {
                continue;
            };
            if path.starts_with(&at) {
                layers.push((at.components().count(), entry));
            }
        }
    }
    layers.sort_by_key(|(depth, _)| *depth);
    let mut merged = object.clone();
    merged.remove("overrides");
    for (_, layer) in layers {
        if let Some(dialect) = layer.get("dialect") {
            // A version given for another dialect says nothing about this one.
            if merged.get("dialect") != Some(dialect) {
                merged.remove("version");
                merged.remove("sqlMode");
            }
        }
        for key in PER_PATH {
            if let Some(value) = layer.get(key) {
                merged.insert(key.to_string(), value.clone());
            }
        }
    }
    let dialect_set = merged
        .get("dialect")
        .and_then(Value::as_str)
        .and_then(sql_embed::Dialect::parse)
        .is_some();
    let (settings, problems) = sql_embed::Settings::from_json(&Value::Object(merged));
    let schema = settings.schema.as_deref().and_then(|text| resolve_path(text, base));
    Resolved {
        enabled: object.get("enabled").and_then(Value::as_bool).unwrap_or(true),
        detection: detection(object.get("detection")),
        settings,
        dialect_set,
        schema,
        problems,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sql_embed::{Dialect, Version};

    #[test]
    fn reads_the_settings_and_the_override_of_a_path() {
        let value = json!({
            "dialect": "mysql",
            "version": "8.4",
            "schema": "db/schema.json",
            "inspections": { "missing-where": "off" },
            "detection": { "heuristic": false, "threshold": 0.9 },
            "overrides": [
                { "path": "legacy", "dialect": "mariadb" },
                { "path": "legacy/reports", "version": "10.6", "schema": "/srv/reports.json" },
                { "path": "other", "dialect": "postgres" }
            ]
        });
        let base = Path::new("/work");
        let top = resolve(Some(&value), Some(Path::new("/work/src/A.php")), Some(base));
        assert!(top.enabled && top.dialect_set);
        assert_eq!(top.settings.target().dialect, Dialect::Mysql);
        assert_eq!(top.settings.version, Version::parse("8.4"));
        assert_eq!(top.schema, Some(PathBuf::from("/work/db/schema.json")));
        assert!(!top.detection.heuristic && top.detection.sinks);
        assert!((top.detection.threshold - 0.9).abs() < f32::EPSILON);
        assert!(!top.settings.inspections.is_empty());
        let legacy = resolve(Some(&value), Some(Path::new("/work/legacy/A.php")), Some(base));
        assert_eq!(legacy.settings.dialect, Dialect::Mariadb);
        assert_eq!(
            legacy.settings.version, None,
            "a MySQL version says nothing about MariaDB"
        );
        let reports = resolve(Some(&value), Some(Path::new("/work/legacy/reports/A.php")), Some(base));
        assert_eq!(reports.settings.dialect, Dialect::Mariadb);
        assert_eq!(reports.settings.version, Version::parse("10.6"));
        assert_eq!(reports.schema, Some(PathBuf::from("/srv/reports.json")));
        let nothing = resolve(None, None, None);
        assert!(nothing.enabled && !nothing.dialect_set);
        assert_eq!(nothing.settings.dialect, Dialect::Generic);
        assert_eq!(nothing.detection, Detection::default());
        let off = resolve(Some(&json!({ "enabled": false, "dialect": "oracle" })), None, None);
        assert!(!off.enabled && !off.dialect_set);
        assert_eq!(off.problems.len(), 1);
    }
}
