//! Which words each file of a project contains, so a search for a name reads only the files that may
//! hold it. A word is a run of identifier characters, whatever it is in the file: a name, a variable
//! without its `$`, a word of a doc comment. That is more than the names, which is the point: a
//! filter that never misses a file.
//!
//! A run that also holds `.`, `-`, `:` or `/` (`admin.users.index`, `mail::welcome`, `emails/x`) is
//! kept whole besides its words, so a search for a route name, a config key or a view reads only the
//! files that write the whole string. The words are kept in a file per project between runs, by the
//! size and modification time of each source file, so a later start reads only what changed.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use xxhash_rust::const_xxh3::xxh3_64 as const_hash;

use crate::cache::Stamp;

const MAGIC: u32 = 0x5048_5057;

/// Changes with the way words are read, so a file written by another build is read again.
const SCHEMA: u64 = const_hash(include_bytes!("words.rs"));

/// The sorted hashes of the lowercase words of one file, and of its compound runs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileWords(Box<[u32]>);

fn hash_word(word: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in word {
        hash ^= u32::from(byte.to_ascii_lowercase());
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

fn is_separator(byte: u8) -> bool {
    matches!(byte, b'.' | b'-' | b':' | b'/')
}

/// Calls `found` with the hash of every word of a text and of every compound run, the separators at
/// either end of a run left off. Each tail of a run that starts at a word is a run too, so the
/// `articles.summary` of `<x-articles.summary>` is found.
fn scan(text: &str, mut found: impl FnMut(u32)) {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut starts: Vec<usize> = Vec::new();
    while index < bytes.len() {
        let byte = bytes[index];
        if !is_word_byte(byte) && !is_separator(byte) {
            index += 1;
            continue;
        }
        starts.clear();
        while index < bytes.len() && (is_word_byte(bytes[index]) || is_separator(bytes[index])) {
            if is_word_byte(bytes[index]) {
                let word_start = index;
                while index < bytes.len() && is_word_byte(bytes[index]) {
                    index += 1;
                }
                found(hash_word(&bytes[word_start..index]));
                starts.push(word_start);
            } else {
                index += 1;
            }
        }
        if starts.len() > 1 {
            let end = bytes[..index]
                .iter()
                .rposition(|byte| !is_separator(*byte))
                .map_or(index, |at| at + 1);
            for start in &starts[..starts.len() - 1] {
                found(hash_word(&bytes[*start..end]));
            }
        }
    }
}

/// The hashes a file must hold to contain a text: the text's own compound runs and words, where a
/// word that is part of a compound run is left to the run.
fn needed(text: &str) -> Vec<u32> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !is_word_byte(byte) && !is_separator(byte) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && (is_word_byte(bytes[index]) || is_separator(bytes[index])) {
            index += 1;
        }
        let run = &bytes[start..index];
        let Some(first) = run.iter().position(|byte| !is_separator(*byte)) else {
            continue;
        };
        let last = run
            .iter()
            .rposition(|byte| !is_separator(*byte))
            .map_or(run.len(), |at| at + 1);
        out.push(hash_word(&run[first..last]));
    }
    out
}

impl FileWords {
    pub fn new(text: &str) -> FileWords {
        let mut hashes: Vec<u32> = Vec::new();
        scan(text, |hash| hashes.push(hash));
        hashes.sort_unstable();
        hashes.dedup();
        FileWords(hashes.into_boxed_slice())
    }

    /// Whether the file may contain a word, or a string such as `admin.users.index` or `Welcome
    /// back!`. A hash can collide, so a yes only means look.
    pub fn may_contain(&self, text: &str) -> bool {
        let needed = needed(text);
        !needed.is_empty() && needed.iter().all(|hash| self.0.binary_search(hash).is_ok())
    }
}

struct Entry {
    words: FileWords,
    /// The version on disk the words were read from; `None` for the text of an open document,
    /// which is not kept between runs.
    stamp: Option<Stamp>,
}

/// How a build went: files whose words came from the file kept on disk, and files read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildCounts {
    pub kept: usize,
    pub read: usize,
}

/// The words of every file of a project, built on first use and kept current from then on.
#[derive(Default)]
pub struct WordIndex {
    files: HashMap<PathBuf, Entry>,
    built: bool,
}

impl WordIndex {
    pub fn is_built(&self) -> bool {
        self.built
    }

    /// Reads the words of the files in parallel, taking the ones whose file did not change from the
    /// words file at `kept`, and writes that file again when anything was read. Files that cannot be
    /// read are left out.
    pub fn build(&mut self, paths: Vec<PathBuf>, kept: Option<&Path>) -> BuildCounts {
        let mut stored = kept.map(load).unwrap_or_default();
        let known = stored.len();
        let read: Vec<(PathBuf, Option<Entry>)> = paths
            .into_par_iter()
            .map(|path| {
                let stamp = Stamp::of(&path);
                let reused = stamp.is_some() && stored.get(&path).is_some_and(|entry| entry.stamp == stamp);
                if reused {
                    return (path, None);
                }
                let entry = fs::read(&path).ok().map(|bytes| Entry {
                    words: FileWords::new(&String::from_utf8_lossy(&bytes)),
                    stamp,
                });
                (path, entry)
            })
            .collect();
        let mut counts = BuildCounts::default();
        let mut files = HashMap::with_capacity(read.len());
        for (path, entry) in read {
            match entry {
                Some(entry) => {
                    counts.read += 1;
                    files.insert(path, entry);
                }
                None => {
                    if let Some(entry) = stored.remove(&path) {
                        counts.kept += 1;
                        files.insert(path, entry);
                    }
                }
            }
        }
        self.files = files;
        self.built = true;
        if let Some(kept) = kept {
            if counts.read > 0 || known != counts.kept {
                let _ = self.save(kept);
            }
        }
        counts
    }

    /// Writes the words of the files as they are on disk next to their final place, and renames
    /// the result over it, so a reader never sees half of it.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("tmp");
        {
            let mut out = io::BufWriter::new(fs::File::create(&temporary)?);
            out.write_all(&MAGIC.to_le_bytes())?;
            out.write_all(&SCHEMA.to_le_bytes())?;
            for (file, entry) in &self.files {
                let Some(stamp) = entry.stamp else {
                    continue;
                };
                let name = file.to_string_lossy();
                out.write_all(&(name.len() as u32).to_le_bytes())?;
                out.write_all(name.as_bytes())?;
                out.write_all(&stamp.size.to_le_bytes())?;
                out.write_all(&stamp.mtime_ns.to_le_bytes())?;
                out.write_all(&(entry.words.0.len() as u32).to_le_bytes())?;
                for hash in &entry.words.0 {
                    out.write_all(&hash.to_le_bytes())?;
                }
            }
            out.flush()?;
        }
        fs::rename(&temporary, path)
    }

    pub fn update(&mut self, path: &Path, text: &str) {
        if self.built {
            self.files.insert(
                path.to_path_buf(),
                Entry {
                    words: FileWords::new(text),
                    stamp: None,
                },
            );
        }
    }

    /// Reads a file again after it changed on disk, or forgets it when it is gone.
    pub fn update_from_disk(&mut self, path: &Path) {
        if !self.built {
            return;
        }
        let stamp = Stamp::of(path);
        match fs::read(path) {
            Ok(bytes) => {
                self.files.insert(
                    path.to_path_buf(),
                    Entry {
                        words: FileWords::new(&String::from_utf8_lossy(&bytes)),
                        stamp,
                    },
                );
            }
            Err(_) => self.remove(path),
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }

    /// The files that may contain a word or a string, given in lowercase.
    pub fn candidates(&self, word: &str) -> Vec<PathBuf> {
        let needed = needed(word);
        if needed.is_empty() {
            return Vec::new();
        }
        self.files
            .iter()
            .filter(|(_, entry)| needed.iter().all(|hash| entry.words.0.binary_search(hash).is_ok()))
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The number of hashes held, for measuring.
    pub fn hash_count(&self) -> usize {
        self.files.values().map(|entry| entry.words.0.len()).sum()
    }
}

/// The words file at a path; an unreadable one, or one of another build, is empty.
fn load(path: &Path) -> HashMap<PathBuf, Entry> {
    let Ok(bytes) = fs::read(path) else {
        return HashMap::new();
    };
    parse_words_file(&bytes).unwrap_or_default()
}

fn parse_words_file(bytes: &[u8]) -> Option<HashMap<PathBuf, Entry>> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.u32()? != MAGIC || reader.u64()? != SCHEMA {
        return None;
    }
    let mut out = HashMap::new();
    while reader.at < bytes.len() {
        let length = reader.u32()? as usize;
        let name = std::str::from_utf8(reader.take(length)?).ok()?;
        let stamp = Stamp {
            size: reader.u64()?,
            mtime_ns: reader.u64()?,
        };
        let count = reader.u32()? as usize;
        let raw = reader.take(count.checked_mul(4)?)?;
        let hashes: Box<[u32]> = raw
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        out.insert(
            PathBuf::from(name),
            Entry {
                words: FileWords(hashes),
                stamp: Some(stamp),
            },
        );
    }
    Some(out)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(length)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_words_without_regard_to_case_or_dollar() {
        let words = FileWords::new("<?php $userName = new Foo\\Bar(); // see handle_it");
        assert!(words.may_contain("username"));
        assert!(words.may_contain("foo"));
        assert!(words.may_contain("bar"));
        assert!(words.may_contain("handle_it"));
        assert!(!words.may_contain("baz"));
    }

    #[test]
    fn keeps_compound_runs_whole() {
        let words = FileWords::new("<?php route('admin.users.index'); view(\"mail::welcome\"); // see app.name.");
        assert!(words.may_contain("admin.users.index"));
        assert!(words.may_contain("mail::welcome"));
        assert!(words.may_contain("app.name"));
        assert!(words.may_contain("users"));
        assert!(!words.may_contain("admin.users"));
        assert!(words.may_contain("users.index"));
        assert!(FileWords::new("<x-articles.summary :a=\"$b\" />").may_contain("articles.summary"));
        assert!(!words.may_contain("admin.posts.index"));
    }

    #[test]
    fn a_phrase_needs_every_word() {
        let words = FileWords::new("<?php __('Welcome back, friend');");
        assert!(words.may_contain("welcome back"));
        assert!(words.may_contain("Welcome back, friend"));
        assert!(!words.may_contain("welcome home"));
        assert!(!words.may_contain("..."));
    }

    #[test]
    fn keeps_the_words_between_runs() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let first = dir.path().join("a.php");
        let second = dir.path().join("b.php");
        fs::write(&first, "<?php route('home.page');").expect("written");
        fs::write(&second, "<?php config('app.name');").expect("written");
        let kept = dir.path().join("cache/words.bin");
        let paths = vec![first.clone(), second.clone()];

        let mut index = WordIndex::default();
        assert_eq!(
            index.build(paths.clone(), Some(&kept)),
            BuildCounts { kept: 0, read: 2 }
        );
        assert_eq!(index.candidates("home.page"), vec![first.clone()]);

        let mut again = WordIndex::default();
        assert_eq!(
            again.build(paths.clone(), Some(&kept)),
            BuildCounts { kept: 2, read: 0 }
        );
        assert_eq!(again.candidates("app.name"), vec![second.clone()]);

        fs::write(&second, "<?php config('app.timezone'); // longer").expect("written");
        let mut changed = WordIndex::default();
        assert_eq!(changed.build(paths, Some(&kept)), BuildCounts { kept: 1, read: 1 });
        assert!(changed.candidates("app.name").is_empty());
        assert_eq!(changed.candidates("app.timezone"), vec![second]);

        fs::write(&kept, b"garbage").expect("written");
        assert!(load(&kept).is_empty());
    }
}
