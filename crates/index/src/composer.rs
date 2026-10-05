//! Composer metadata: the language level a project asks for, the extensions it requires and the
//! autoload maps that say which files define which names.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use php_syntax::PhpVersion;
use serde_json::Value;

/// What `autoload` and `autoload-dev` of a package say, with directories made absolute.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Autoload {
    /// A namespace prefix with a trailing backslash (or empty) and the directories it maps to.
    pub psr4: Vec<(String, PathBuf)>,
    pub psr0: Vec<(String, PathBuf)>,
    /// Directories or single files whose declarations Composer finds by scanning.
    pub classmap: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub exclude: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub path: PathBuf,
    pub autoload: Autoload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composer {
    pub root: PathBuf,
    pub vendor_dir: PathBuf,
    /// From `config.platform.php`, else the lower bound of `require.php`.
    pub php_level: Option<PhpVersion>,
    /// Required extensions, lowercase and without the `ext-` prefix.
    pub extensions: Vec<String>,
    /// The autoload maps of the project itself, `autoload-dev` included.
    pub autoload: Autoload,
    /// The installed packages, from `vendor/composer/installed.json`.
    pub packages: Vec<Package>,
    /// `composer.json` requires packages, so a project whose `packages` are empty is not installed.
    pub requires_packages: bool,
    /// The packages `composer.json` requires, `require-dev` included, lowercase.
    pub requires: Vec<String>,
}

impl Composer {
    /// Reads `composer.json` in a folder and, when it is there, the installed packages. `None`
    /// when the folder has no readable `composer.json`.
    pub fn load(root: &Path) -> Option<Composer> {
        let text = std::fs::read_to_string(root.join("composer.json")).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        Some(Composer::from_json(root, &json))
    }

    pub fn from_json(root: &Path, json: &Value) -> Composer {
        let vendor_dir = json
            .pointer("/config/vendor-dir")
            .and_then(Value::as_str)
            .map_or_else(|| root.join("vendor"), |dir| root.join(dir.trim_end_matches('/')));
        let platform = json
            .pointer("/config/platform/php")
            .and_then(Value::as_str)
            .and_then(parse_exact_version);
        let required = json
            .pointer("/require/php")
            .and_then(Value::as_str)
            .and_then(lower_bound);
        let mut extensions = BTreeSet::new();
        for section in ["require", "require-dev"] {
            if let Some(map) = json.get(section).and_then(Value::as_object) {
                for key in map.keys() {
                    if let Some(extension) = key.strip_prefix("ext-") {
                        extensions.insert(extension.to_ascii_lowercase());
                    }
                }
            }
        }
        let requires_packages = ["require", "require-dev"].iter().any(|section| {
            json.get(section).and_then(Value::as_object).is_some_and(|map| {
                map.keys()
                    .any(|key| key != "php" && !key.starts_with("ext-") && !key.starts_with("lib-"))
            })
        });
        let requires = ["require", "require-dev"]
            .iter()
            .filter_map(|section| json.get(section).and_then(Value::as_object))
            .flat_map(|map| map.keys().map(|key| key.to_ascii_lowercase()))
            .collect();
        let mut autoload = parse_autoload(json.get("autoload"), root);
        merge(&mut autoload, parse_autoload(json.get("autoload-dev"), root));
        let packages = read_installed(&vendor_dir);
        Composer {
            root: root.to_path_buf(),
            vendor_dir,
            php_level: platform.or(required),
            extensions: extensions.into_iter().collect(),
            autoload,
            packages,
            requires_packages,
            requires,
        }
    }

    /// Whether the project requires a package or has it installed.
    pub fn has_package(&self, name: &str) -> bool {
        self.requires.iter().any(|required| required == name)
            || self
                .packages
                .iter()
                .any(|package| package.name.eq_ignore_ascii_case(name))
    }

    /// Whether the project requires or has installed a package of a vendor, such as `illuminate`.
    pub fn has_vendor(&self, vendor: &str) -> bool {
        let prefix = format!("{vendor}/");
        self.requires.iter().any(|required| required.starts_with(&prefix))
            || self
                .packages
                .iter()
                .any(|package| package.name.to_ascii_lowercase().starts_with(&prefix))
    }

    /// The files that may define a class, by the PSR-4 and PSR-0 maps of the project and its packages.
    pub fn class_candidates(&self, class: &str) -> Vec<PathBuf> {
        let class = class.trim_start_matches('\\');
        let mut out = Vec::new();
        let maps = std::iter::once(&self.autoload).chain(self.packages.iter().map(|package| &package.autoload));
        for autoload in maps {
            let mut psr4: Vec<&(String, PathBuf)> = autoload
                .psr4
                .iter()
                .filter(|(prefix, _)| class.starts_with(prefix.as_str()))
                .collect();
            psr4.sort_by_key(|(prefix, _)| std::cmp::Reverse(prefix.len()));
            for (prefix, dir) in psr4 {
                let rest = class[prefix.len()..].replace('\\', "/");
                if !rest.is_empty() {
                    out.push(dir.join(format!("{rest}.php")));
                }
            }
            for (prefix, dir) in &autoload.psr0 {
                if !class.starts_with(prefix.as_str()) {
                    continue;
                }
                let (namespace, short) = class.rsplit_once('\\').unwrap_or(("", class));
                let mut path = namespace.replace('\\', "/");
                if !path.is_empty() {
                    path.push('/');
                }
                path.push_str(&short.replace('_', "/"));
                out.push(dir.join(format!("{path}.php")));
            }
        }
        out
    }

    /// The directories and files of the packages that Composer would load: where the declarations
    /// of `vendor/` are, and the two classes Composer writes there itself. The project's own files
    /// are not among them.
    pub fn vendor_roots(&self) -> Vec<PathBuf> {
        let mut roots: BTreeSet<PathBuf> = BTreeSet::new();
        for generated in ["InstalledVersions.php", "ClassLoader.php"] {
            let path = self.vendor_dir.join("composer").join(generated);
            if path.is_file() {
                roots.insert(path);
            }
        }
        for package in &self.packages {
            let autoload = &package.autoload;
            roots.extend(autoload.psr4.iter().map(|(_, dir)| dir.clone()));
            roots.extend(autoload.psr0.iter().map(|(_, dir)| dir.clone()));
            roots.extend(autoload.classmap.iter().cloned());
            roots.extend(autoload.files.iter().cloned());
        }
        roots.into_iter().collect()
    }

    pub fn excluded(&self, path: &Path) -> bool {
        self.packages
            .iter()
            .flat_map(|package| package.autoload.exclude.iter())
            .any(|excluded| path.starts_with(excluded))
    }
}

fn merge(into: &mut Autoload, other: Autoload) {
    into.psr4.extend(other.psr4);
    into.psr0.extend(other.psr0);
    into.classmap.extend(other.classmap);
    into.files.extend(other.files);
    into.exclude.extend(other.exclude);
}

fn parse_autoload(value: Option<&Value>, base: &Path) -> Autoload {
    let mut autoload = Autoload::default();
    let Some(value) = value else {
        return autoload;
    };
    for (key, target) in [("psr-4", true), ("psr-0", false)].map(|(key, psr4)| (key, psr4)) {
        let Some(map) = value.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (prefix, dirs) in map {
            let prefix = if target && !prefix.is_empty() && !prefix.ends_with('\\') {
                format!("{prefix}\\")
            } else {
                prefix.clone()
            };
            for dir in strings(dirs) {
                let entry = (prefix.clone(), base.join(dir.trim_end_matches('/')));
                if target {
                    autoload.psr4.push(entry);
                } else {
                    autoload.psr0.push(entry);
                }
            }
        }
    }
    if let Some(classmap) = value.get("classmap") {
        autoload.classmap.extend(
            strings(classmap)
                .into_iter()
                .map(|path| base.join(path.trim_end_matches('/'))),
        );
    }
    if let Some(files) = value.get("files") {
        autoload
            .files
            .extend(strings(files).into_iter().map(|path| base.join(path)));
    }
    if let Some(exclude) = value.get("exclude-from-classmap") {
        autoload.exclude.extend(
            strings(exclude)
                .into_iter()
                .map(|path| base.join(path.trim_matches('*').trim_matches('/'))),
        );
    }
    autoload
}

fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// `vendor/composer/installed.json`, in the version 1 form (a list) and the version 2 form (an
/// object with `packages`).
fn read_installed(vendor_dir: &Path) -> Vec<Package> {
    let composer_dir = vendor_dir.join("composer");
    let Ok(text) = std::fs::read_to_string(composer_dir.join("installed.json")) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let list = match &json {
        Value::Array(list) => list.as_slice(),
        Value::Object(object) => object
            .get("packages")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice),
        _ => &[],
    };
    list.iter()
        .filter_map(|package| {
            let name = package.get("name")?.as_str()?.to_string();
            let path = match package.get("install-path").and_then(Value::as_str) {
                Some(relative) => normalize(&composer_dir.join(relative)),
                None => vendor_dir.join(&name),
            };
            let mut autoload = parse_autoload(package.get("autoload"), &path);
            if let Some(target) = package.get("target-dir").and_then(Value::as_str) {
                for (_, dir) in autoload.psr0.iter_mut().chain(autoload.psr4.iter_mut()) {
                    *dir = dir.join(target);
                }
            }
            Some(Package { name, path, autoload })
        })
        .collect()
}

/// Resolves `..` and `.` without touching the file system.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `8.5`, `8.1.2` or `8.1.*` as a platform version.
fn parse_exact_version(text: &str) -> Option<PhpVersion> {
    version_of(text.trim().trim_start_matches('v'))
}

fn version_of(text: &str) -> Option<PhpVersion> {
    let cleaned: Vec<&str> = text
        .split('.')
        .map(|part| {
            if part == "*" || part.eq_ignore_ascii_case("x") {
                "0"
            } else {
                part
            }
        })
        .collect();
    PhpVersion::parse(&cleaned.join(".")).filter(|version| version.major > 0)
}

/// The oldest version a constraint such as `^8.1 || ^8.2`, `>=8.1 <8.4` or `~8.1.0` admits.
pub fn lower_bound(constraint: &str) -> Option<PhpVersion> {
    let mut lowest: Option<PhpVersion> = None;
    for alternative in constraint.split("||").flat_map(|part| part.split('|')) {
        let tokens: Vec<&str> = alternative
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|token| !token.is_empty())
            .collect();
        let mut bound: Option<PhpVersion> = None;
        for (index, token) in tokens.iter().enumerate() {
            if *token == "-" {
                continue;
            }
            // The upper end of `8.1 - 8.3`.
            if index >= 2 && tokens[index - 1] == "-" {
                continue;
            }
            let version = if let Some(rest) = token.strip_prefix(">=") {
                version_of(rest)
            } else if token.starts_with('<') || token.starts_with("!=") {
                None
            } else {
                let rest = token.trim_start_matches(['>', '^', '~', '=', 'v']);
                version_of(rest)
            };
            if let Some(version) = version {
                bound = Some(bound.map_or(version, |current| current.max(version)));
            }
        }
        if let Some(bound) = bound {
            lowest = Some(lowest.map_or(bound, |current| current.min(bound)));
        }
    }
    lowest
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_the_lower_bound_of_a_constraint() {
        assert_eq!(lower_bound("^8.1"), Some(PhpVersion::V8_1));
        assert_eq!(lower_bound(">=8.2"), Some(PhpVersion::V8_2));
        assert_eq!(lower_bound("^8.2 || ^8.3"), Some(PhpVersion::V8_2));
        assert_eq!(lower_bound("^7.4|^8.0"), Some(PhpVersion::V7_4));
        assert_eq!(lower_bound(">=8.1 <8.4"), Some(PhpVersion::V8_1));
        assert_eq!(lower_bound("~8.1.0"), Some(PhpVersion::V8_1));
        assert_eq!(lower_bound("8.3.*"), Some(PhpVersion::V8_3));
        assert_eq!(lower_bound("8.1 - 8.3"), Some(PhpVersion::V8_1));
        assert_eq!(lower_bound(">=8.1, <8.5"), Some(PhpVersion::V8_1));
        assert_eq!(lower_bound("*"), None);
    }

    #[test]
    fn the_platform_overrides_the_requirement() {
        let json = json!({
            "require": { "php": "^8.1", "ext-mbstring": "*", "ext-Redis": "*" },
            "require-dev": { "ext-pcov": "*" },
            "config": { "platform": { "php": "8.3.1" } },
            "autoload": { "psr-4": { "App\\": "src/", "Lib": ["lib/a", "lib/b"] }, "files": ["src/helpers.php"] },
            "autoload-dev": { "psr-4": { "Tests\\": "tests/" } }
        });
        let composer = Composer::from_json(Path::new("/p"), &json);
        assert_eq!(composer.php_level, Some(PhpVersion::V8_3));
        assert_eq!(composer.extensions, vec!["mbstring", "pcov", "redis"]);
        assert_eq!(composer.autoload.psr4.len(), 4);
        assert_eq!(
            composer.autoload.psr4[0],
            ("App\\".to_string(), PathBuf::from("/p/src"))
        );
        assert_eq!(composer.autoload.psr4[1].0, "Lib\\");
        assert_eq!(composer.autoload.files, vec![PathBuf::from("/p/src/helpers.php")]);
        let without_platform = Composer::from_json(Path::new("/p"), &json!({ "require": { "php": "^8.1" } }));
        assert_eq!(without_platform.php_level, Some(PhpVersion::V8_1));
        let none = Composer::from_json(Path::new("/p"), &json!({}));
        assert_eq!(none.php_level, None);
    }

    #[test]
    fn maps_class_names_to_candidate_files() {
        let composer = Composer {
            root: PathBuf::from("/p"),
            vendor_dir: PathBuf::from("/p/vendor"),
            php_level: None,
            extensions: Vec::new(),
            autoload: Autoload {
                psr4: vec![
                    ("App\\".into(), PathBuf::from("/p/src")),
                    ("App\\Models\\".into(), PathBuf::from("/p/models")),
                ],
                ..Autoload::default()
            },
            requires_packages: false,
            requires: Vec::new(),
            packages: vec![Package {
                name: "old/lib".into(),
                path: PathBuf::from("/p/vendor/old/lib"),
                autoload: Autoload {
                    psr0: vec![("Old_".into(), PathBuf::from("/p/vendor/old/lib/src"))],
                    ..Autoload::default()
                },
            }],
        };
        assert_eq!(
            composer.class_candidates("App\\Models\\User"),
            vec![
                PathBuf::from("/p/models/User.php"),
                PathBuf::from("/p/src/Models/User.php")
            ]
        );
        assert_eq!(
            composer.class_candidates("Old_Thing_Here"),
            vec![PathBuf::from("/p/vendor/old/lib/src/Old/Thing/Here.php")]
        );
    }

    #[test]
    fn the_classes_composer_writes_into_vendor_are_loaded() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let vendor = dir.path().join("vendor/composer");
        std::fs::create_dir_all(&vendor).expect("created");
        std::fs::write(
            vendor.join("InstalledVersions.php"),
            "<?php namespace Composer; class InstalledVersions {}",
        )
        .expect("written");
        std::fs::write(dir.path().join("composer.json"), "{}").expect("written");
        let composer = Composer::load(dir.path()).expect("composer.json is there");
        assert_eq!(composer.vendor_roots(), vec![vendor.join("InstalledVersions.php")]);
    }

    #[test]
    fn reads_installed_packages_in_both_formats() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let vendor = dir.path().join("vendor");
        std::fs::create_dir_all(vendor.join("composer")).expect("created");
        std::fs::write(
            vendor.join("composer/installed.json"),
            r#"{"packages":[{"name":"a/b","install-path":"../a/b","autoload":{"psr-4":{"A\\B\\":"src/"}}}]}"#,
        )
        .expect("written");
        let packages = read_installed(&vendor);
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].path, vendor.join("a/b"));
        assert_eq!(packages[0].autoload.psr4[0].1, vendor.join("a/b/src"));

        std::fs::write(
            vendor.join("composer/installed.json"),
            r#"[{"name":"c/d","autoload":{"classmap":["lib/"]}}]"#,
        )
        .expect("written");
        let packages = read_installed(&vendor);
        assert_eq!(packages[0].autoload.classmap, vec![vendor.join("c/d/lib")]);
    }
}
