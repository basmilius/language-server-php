//! Which words each file of a project contains, so a search for a name reads only the files that may
//! hold it. A word is a run of identifier characters, whatever it is in the file: a name, a variable
//! without its `$`, a word of a doc comment. That is more than the names, which is the point: a
//! filter that never misses a file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

/// The sorted hashes of the lowercase words of one file.
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

impl FileWords {
    pub fn new(text: &str) -> FileWords {
        let bytes = text.as_bytes();
        let mut hashes: Vec<u32> = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            if !is_word_byte(bytes[index]) {
                index += 1;
                continue;
            }
            let start = index;
            while index < bytes.len() && is_word_byte(bytes[index]) {
                index += 1;
            }
            hashes.push(hash_word(&bytes[start..index]));
        }
        hashes.sort_unstable();
        hashes.dedup();
        FileWords(hashes.into_boxed_slice())
    }

    /// Whether the file may contain a word. A hash can collide, so a yes only means look.
    pub fn may_contain(&self, word: &str) -> bool {
        self.0.binary_search(&hash_word(word.as_bytes())).is_ok()
    }
}

/// The words of every file of a project, built on first use and kept current from then on.
#[derive(Default)]
pub struct WordIndex {
    files: HashMap<PathBuf, FileWords>,
    built: bool,
}

impl WordIndex {
    pub fn is_built(&self) -> bool {
        self.built
    }

    /// Reads the files in parallel. Files that cannot be read are left out.
    pub fn build(&mut self, paths: Vec<PathBuf>) {
        self.files = paths
            .into_par_iter()
            .filter_map(|path| {
                let bytes = std::fs::read(&path).ok()?;
                let words = FileWords::new(&String::from_utf8_lossy(&bytes));
                Some((path, words))
            })
            .collect();
        self.built = true;
    }

    pub fn update(&mut self, path: &Path, text: &str) {
        if self.built {
            self.files.insert(path.to_path_buf(), FileWords::new(text));
        }
    }

    /// Reads a file again after it changed on disk, or forgets it when it is gone.
    pub fn update_from_disk(&mut self, path: &Path) {
        if !self.built {
            return;
        }
        match std::fs::read(path) {
            Ok(bytes) => self.update(path, &String::from_utf8_lossy(&bytes)),
            Err(_) => self.remove(path),
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }

    pub fn candidates(&self, word: &str) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|(_, words)| words.may_contain(word))
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
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
}
