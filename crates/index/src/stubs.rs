//! The standard library as the index knows it: the stubs of JetBrains/phpstorm-stubs (Apache 2.0).
//! They are not part of this repository or of the binary. The server fetches one pinned commit on
//! first run into its storage path, so each run reads the same declarations.

use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

/// The commit of phpstorm-stubs the index is built against.
pub const STUBS_COMMIT: &str = "e4f5f6c3de39f3bab3e9f3fca4b8cdb8b061e681";

const STUBS_URL: &str = "https://codeload.github.com/JetBrains/phpstorm-stubs/tar.gz/";

/// Extension folders that are offered without a project asking for them: what a regular PHP
/// distribution has built in or loaded by default.
const DEFAULT_EXTENSIONS: &[&str] = &[
    "core",
    "standard",
    "spl",
    "reflection",
    "date",
    "pcre",
    "json",
    "hash",
    "random",
    "ctype",
    "filter",
    "iconv",
    "mbstring",
    "libxml",
    "dom",
    "xml",
    "xmlreader",
    "xmlwriter",
    "simplexml",
    "session",
    "tokenizer",
    "zlib",
    "curl",
    "openssl",
    "pdo",
    "phar",
    "posix",
    "readline",
    "sqlite3",
    "fileinfo",
    "intl",
    "mysqli",
    "gd",
    "zip",
    "bcmath",
    "sockets",
    "pcntl",
    "calendar",
    "exif",
    "gettext",
    "soap",
    "libsodium",
    "superglobals",
    "zend",
    "ftp",
    "gmp",
    "xsl",
    "zend opcache",
];

/// Folders of the stubs repository that are not extensions.
const NOT_EXTENSIONS: &[&str] = &["tests", "meta"];

/// Where the stubs of this commit live inside a storage folder.
pub fn stubs_dir(storage: &Path) -> PathBuf {
    storage.join("stubs").join(STUBS_COMMIT)
}

/// The folder holding fetched stubs, if there is one.
pub fn locate(storage: &Path) -> Option<PathBuf> {
    let dir = stubs_dir(storage);
    dir.join(".complete").exists().then_some(dir)
}

/// Downloads the pinned commit into the storage folder and unpacks the `.php` files and the license.
/// The folder only counts once it is complete, so an interrupted download is started over.
pub fn fetch(storage: &Path) -> Result<PathBuf, String> {
    let target = stubs_dir(storage);
    if target.join(".complete").exists() {
        return Ok(target);
    }
    let partial = target.with_extension("partial");
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|error| error.to_string())?;

    let url = format!("{STUBS_URL}{STUBS_COMMIT}");
    let mut response = ureq::get(&url).call().map_err(|error| format!("{url}: {error}"))?;
    let reader = response.body_mut().as_reader();
    unpack(GzDecoder::new(reader), &partial).map_err(|error| error.to_string())?;

    std::fs::write(partial.join(".commit"), STUBS_COMMIT).map_err(|error| error.to_string())?;
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&partial, &target).map_err(|error| error.to_string())?;
    std::fs::write(target.join(".complete"), "").map_err(|error| error.to_string())?;
    Ok(target)
}

fn unpack(reader: impl Read, into: &Path) -> std::io::Result<()> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        // The first component is the folder GitHub wraps the commit in.
        let relative: PathBuf = path.components().skip(1).collect();
        let keep =
            relative.extension().is_some_and(|extension| extension == "php") || relative.as_os_str() == "LICENSE";
        if !keep || !entry.header().entry_type().is_file() {
            continue;
        }
        if relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            continue;
        }
        let destination = into.join(&relative);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        entry.unpack(&destination)?;
    }
    Ok(())
}

/// The extension a stub file belongs to: its first folder under the stubs root, lowercased and
/// without spaces or underscores.
pub fn extension_of(stubs_root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(stubs_root).ok()?;
    let first = relative.components().next()?;
    if relative.components().count() < 2 {
        return None;
    }
    Some(normalize_extension(&first.as_os_str().to_string_lossy()))
}

pub fn normalize_extension(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Which extension folders a project sees: the default set plus what its `composer.json` requires.
pub fn selected_extensions(required: &[String]) -> Vec<String> {
    let mut out: Vec<String> = DEFAULT_EXTENSIONS
        .iter()
        .map(|name| normalize_extension(name))
        .collect();
    for name in required {
        let normalized = normalize_extension(name);
        let mapped = match normalized.as_str() {
            "sodium" => "libsodium".to_string(),
            "zendopcache" | "opcache" => "zendopcache".to_string(),
            other if other.starts_with("pdo") => "pdo".to_string(),
            other => other.to_string(),
        };
        if !out.contains(&mapped) {
            out.push(mapped);
        }
    }
    out
}

/// Whether a folder of the stubs holds an extension, as opposed to the repository's own files.
pub fn is_extension_folder(name: &str) -> bool {
    !name.starts_with('.') && !NOT_EXTENSIONS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_extension_of_a_stub_file() {
        let root = Path::new("/s");
        assert_eq!(
            extension_of(root, Path::new("/s/Zend OPcache/x.php")).as_deref(),
            Some("zendopcache")
        );
        assert_eq!(
            extension_of(root, Path::new("/s/standard/basic.php")).as_deref(),
            Some("standard")
        );
        assert_eq!(extension_of(root, Path::new("/s/PhpStormStubsMap.php")), None);
    }

    #[test]
    fn required_extensions_extend_the_default_set() {
        let selected = selected_extensions(&["redis".to_string(), "sodium".to_string(), "pdo_mysql".to_string()]);
        assert!(selected.contains(&"redis".to_string()));
        assert!(selected.contains(&"libsodium".to_string()));
        assert!(selected.contains(&"standard".to_string()));
        assert!(!selected.contains(&"swoole".to_string()));
    }

    #[test]
    fn unpacks_php_files_and_the_license_only() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in [
            ("stubs-abc/standard/basic.php", "<?php function f() {}"),
            ("stubs-abc/LICENSE", "Apache"),
            ("stubs-abc/composer.json", "{}"),
            ("stubs-abc/standard/notes.md", "x"),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, path, body.as_bytes())
                .expect("appended");
        }
        let bytes = builder.into_inner().expect("finished");
        unpack(bytes.as_slice(), dir.path()).expect("unpacked");
        assert!(dir.path().join("standard/basic.php").exists());
        assert!(dir.path().join("LICENSE").exists());
        assert!(!dir.path().join("composer.json").exists());
        assert!(!dir.path().join("standard/notes.md").exists());
    }
}
