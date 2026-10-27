//! The owner manifest: a small file beside the caches of one project or of the stubs that names
//! every file the owner can have and the folder that brought it in. A host reads it to evict the
//! caches of a checkout that is gone or to keep a byte budget, without deriving the cache names
//! itself. The server never deletes a cache.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::cache::path_key;
use crate::project::{cache_file_name, words_file_name};
use crate::stubs::stubs_file_name;

/// Bumped only for a change a host that knows the old shape would misread.
pub const FORMAT: u32 = 1;

/// One writer at a time, so two threads never share the `.partial` file.
static WRITING: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Project,
    Stubs,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub format: u32,
    pub kind: Kind,

    /// The root of the project, or the folder of the stubs.
    pub path: String,

    /// The workspace folder that brought the owner in. The stubs have none, so it is their folder.
    pub folder: String,

    /// Every name the owner can have in the cache folder. Some of them may not exist.
    pub files: Vec<String>,

    pub last_used_ms: u64,
    pub server: String,
}

impl Manifest {
    pub fn project(root: &Path, folder: &Path) -> Manifest {
        Manifest {
            format: FORMAT,
            kind: Kind::Project,
            path: root.to_string_lossy().into_owned(),
            folder: folder.to_string_lossy().into_owned(),
            files: vec![
                cache_file_name(root),
                words_file_name(root, false),
                words_file_name(root, true),
            ],
            last_used_ms: now_ms(),
            server: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn stubs(dir: &Path) -> Manifest {
        Manifest {
            format: FORMAT,
            kind: Kind::Stubs,
            path: dir.to_string_lossy().into_owned(),
            folder: dir.to_string_lossy().into_owned(),
            files: vec![stubs_file_name(dir)],
            last_used_ms: now_ms(),
            server: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn file_name(&self) -> String {
        let key = path_key(Path::new(&self.path));

        match self.kind {
            Kind::Project => format!("owner-{key}.json"),
            Kind::Stubs => format!("owner-stubs-{key}.json"),
        }
    }

    /// Writes the manifest into the cache folder of `storage` through a temporary file, so a reader
    /// never sees half of it.
    pub fn write(&self, storage: &Path) -> io::Result<PathBuf> {
        let dir = storage.join("cache");
        fs::create_dir_all(&dir)?;

        let target = dir.join(self.file_name());
        let partial = dir.join(format!("{}.partial", self.file_name()));
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;

        let _guard = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        fs::write(&partial, bytes)?;

        if let Err(error) = fs::rename(&partial, &target) {
            let _ = fs::remove_file(&partial);

            return Err(error);
        }

        Ok(target)
    }
}

/// Notes a project in the storage. Best effort: a failure goes to stderr and never reaches indexing.
pub fn record_project(storage: &Path, root: &Path, folder: &Path) {
    if root.as_os_str().is_empty() {
        return;
    }

    report(Manifest::project(root, folder).write(storage));
}

/// Notes the stubs in the storage, with the same care as `record_project`.
pub fn record_stubs(storage: &Path, dir: &Path) {
    report(Manifest::stubs(dir).write(storage));
}

fn report(result: io::Result<PathBuf>) {
    if let Err(error) = result {
        eprintln!("php-language-server: could not write an owner manifest: {error}");
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_manifest_names_every_file_the_project_can_have() {
        let storage = tempfile::tempdir().expect("a temp dir");
        let root = Path::new("/work/app");
        let folder = Path::new("/work");

        record_project(storage.path(), root, folder);

        let key = path_key(root);
        let text = fs::read_to_string(storage.path().join(format!("cache/owner-{key}.json"))).expect("written");
        let manifest: Manifest = serde_json::from_str(&text).expect("valid json");

        assert_eq!(manifest.format, 1);
        assert_eq!(manifest.kind, Kind::Project);
        assert_eq!(manifest.path, "/work/app");
        assert_eq!(manifest.folder, "/work");
        assert_eq!(
            manifest.files,
            vec![
                format!("project-{key}.bin"),
                format!("words-{key}.bin"),
                format!("package-words-{key}.bin"),
            ]
        );
        assert!(manifest.last_used_ms > 0);
        assert_eq!(manifest.server, env!("CARGO_PKG_VERSION"));
        assert!(text.contains("\"lastUsedMs\""));
    }

    #[test]
    fn a_stubs_manifest_has_its_own_name_and_cache_file() {
        let storage = tempfile::tempdir().expect("a temp dir");
        let dir = Path::new("/storage/stubs/abc");

        record_stubs(storage.path(), dir);

        let key = path_key(dir);
        let text = fs::read_to_string(storage.path().join(format!("cache/owner-stubs-{key}.json"))).expect("written");
        let manifest: Manifest = serde_json::from_str(&text).expect("valid json");

        assert_eq!(manifest.kind, Kind::Stubs);
        assert_eq!(manifest.path, manifest.folder);
        assert_eq!(manifest.files, vec![format!("stubs-{key}.bin")]);
        assert!(text.contains("\"kind\": \"stubs\""));
    }

    #[test]
    fn a_nested_root_keeps_the_folder_that_brought_it_in() {
        let storage = tempfile::tempdir().expect("a temp dir");
        let folder = Path::new("/work");
        let first = Path::new("/work/a");
        let second = Path::new("/work/b");

        record_project(storage.path(), first, folder);
        record_project(storage.path(), second, folder);

        for root in [first, second] {
            let text = fs::read_to_string(storage.path().join(format!("cache/owner-{}.json", path_key(root))))
                .expect("written");
            let manifest: Manifest = serde_json::from_str(&text).expect("valid json");

            assert_eq!(manifest.path, root.to_string_lossy());
            assert_eq!(manifest.folder, "/work");
        }
    }

    #[test]
    fn writing_again_replaces_the_manifest_and_leaves_no_partial_file() {
        let storage = tempfile::tempdir().expect("a temp dir");
        let root = Path::new("/work/app");

        record_project(storage.path(), root, Path::new("/old"));
        record_project(storage.path(), root, Path::new("/new"));

        let names: Vec<String> = fs::read_dir(storage.path().join("cache"))
            .expect("listed")
            .map(|entry| entry.expect("an entry").file_name().to_string_lossy().into_owned())
            .collect();

        assert_eq!(names, vec![format!("owner-{}.json", path_key(root))]);

        let text = fs::read_to_string(storage.path().join("cache").join(&names[0])).expect("read");
        assert!(text.contains("\"/new\""));
    }

    #[test]
    fn a_failed_write_does_not_panic() {
        let storage = tempfile::tempdir().expect("a temp dir");
        let blocker = storage.path().join("blocked");
        fs::write(&blocker, "a file where the storage folder should be").expect("written");

        record_project(&blocker, Path::new("/work/app"), Path::new("/work"));
        record_stubs(&blocker, Path::new("/stubs"));
    }

    #[test]
    fn the_loose_project_has_no_manifest() {
        let storage = tempfile::tempdir().expect("a temp dir");

        record_project(storage.path(), Path::new(""), Path::new(""));

        assert!(!storage.path().join("cache").exists());
    }
}
