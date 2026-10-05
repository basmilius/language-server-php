//! The cache that keeps extracted declarations between runs. One file per project, keyed by the path
//! of each source file with its size and modification time, and a hash of its content for the case
//! where a tool touched a file without changing it.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use xxhash_rust::const_xxh3::xxh3_64 as const_hash;
use xxhash_rust::xxh3::xxh3_64;

use crate::model::{FileSummary, FileSymbols};
use crate::store::{SymbolSource, SymbolStore};

const MAGIC: u32 = 0x5048_5049;

/// Changes whenever the sources that decide what a cached entry means change, so a cache written by
/// an older build is dropped instead of read as something it is not.
const SCHEMA: u64 = const_hash(include_bytes!("model.rs"))
    ^ const_hash(include_bytes!("types.rs")).rotate_left(1)
    ^ const_hash(include_bytes!("extract.rs")).rotate_left(2)
    ^ const_hash(include_bytes!("phpdoc.rs")).rotate_left(3)
    ^ const_hash(include_bytes!("cache.rs")).rotate_left(4)
    ^ const_hash(include_bytes!("store.rs")).rotate_left(5)
    ^ const_hash(include_bytes!("stub_overlay.php")).rotate_left(6)
    ^ const_hash(include_bytes!("stub_overlay.rs")).rotate_left(7)
    ^ const_hash(include_bytes!("test_facts.rs")).rotate_left(8);

/// What identifies a version of a file on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    pub mtime_ns: u64,
}

impl Stamp {
    pub fn of(path: &Path) -> Option<Stamp> {
        let metadata = fs::metadata(path).ok()?;
        let modified = metadata.modified().ok()?;
        let nanos = modified.duration_since(UNIX_EPOCH).ok()?.as_nanos();
        Some(Stamp {
            size: metadata.len(),
            mtime_ns: u64::try_from(nanos).unwrap_or(u64::MAX),
        })
    }
}

/// A stable name for a path, for the name of a cache file.
pub fn path_key(path: &Path) -> String {
    format!("{:016x}", xxh3_64(path.to_string_lossy().as_bytes()))
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    xxh3_64(bytes)
}

/// One file of the cache as it is written in the index at its head.
#[derive(Serialize, Deserialize)]
struct StoredHead {
    path: String,
    stamp: Stamp,
    hash: u64,
    /// Where the declarations start, counted from the start of the payloads.
    offset: u64,
    length: u32,
    summary: FileSummary,
}

#[derive(Serialize, Deserialize)]
struct Header {
    magic: u32,
    schema: u64,
}

/// What the cache knows of a file: which version of it, what it declares by name, and where its
/// declarations are.
#[derive(Clone, Debug)]
pub struct CachedFile {
    pub stamp: Stamp,
    pub hash: u64,
    /// Absolute in the cache file.
    pub offset: u64,
    pub length: u32,
    pub summary: Arc<FileSummary>,
}

impl CachedFile {
    pub fn source(&self, store: &Arc<SymbolStore>) -> SymbolSource {
        SymbolSource::Cached {
            store: store.clone(),
            offset: self.offset,
            length: self.length,
        }
    }
}

/// The declarations of a file that goes into the cache file.
pub enum Payload {
    Bytes(Vec<u8>),
    /// Declarations that are in an older cache file already.
    Stored {
        store: Arc<SymbolStore>,
        offset: u64,
        length: u32,
    },
}

impl Payload {
    fn length(&self) -> usize {
        match self {
            Payload::Bytes(bytes) => bytes.len(),
            Payload::Stored { length, .. } => *length as usize,
        }
    }
}

/// A file to write into the cache.
pub struct Entry {
    pub path: String,
    pub stamp: Stamp,
    pub hash: u64,
    pub summary: Arc<FileSummary>,
    pub payload: Payload,
}

impl Entry {
    /// The entry of a file whose declarations were just extracted.
    pub fn new(
        path: &Path,
        stamp: Stamp,
        hash: u64,
        summary: Arc<FileSummary>,
        symbols: &FileSymbols,
    ) -> Option<Entry> {
        Some(Entry {
            path: path.to_string_lossy().into_owned(),
            stamp,
            hash,
            summary,
            payload: Payload::Bytes(postcard::to_stdvec(symbols).ok()?),
        })
    }
}

/// What a cache file was saved as: the file opened for reading, and where each path went.
pub struct Saved {
    pub store: Arc<SymbolStore>,
    pub places: HashMap<String, (u64, u32)>,
}

#[derive(Default)]
pub struct Cache {
    pub files: HashMap<String, CachedFile>,
    /// The cache file the declarations are read from. `None` for a cache that holds nothing.
    pub store: Option<Arc<SymbolStore>>,
}

impl Cache {
    /// Reads the index of a cache file and leaves the declarations on disk. An unreadable or
    /// outdated cache is an empty one.
    pub fn load(path: &Path) -> Cache {
        Cache::try_load(path).unwrap_or_default()
    }

    fn try_load(path: &Path) -> Option<Cache> {
        let store = Arc::new(SymbolStore::open(path).ok()?);
        let probe = store.read(0, 64.min(fs::metadata(path).ok()?.len() as usize)).ok()?;
        let (header, rest) = postcard::take_from_bytes::<Header>(&probe).ok()?;
        if header.magic != MAGIC || header.schema != SCHEMA {
            return None;
        }
        let header_length = probe.len() - rest.len();
        let index_length = u64::from_le_bytes(store.read(header_length as u64, 8).ok()?.try_into().ok()?);
        let index_start = header_length as u64 + 8;
        let index = store.read(index_start, usize::try_from(index_length).ok()?).ok()?;
        let heads: Vec<StoredHead> = postcard::from_bytes(&index).ok()?;
        let base = index_start + index_length;
        let files = heads
            .into_iter()
            .map(|head| {
                (
                    head.path,
                    CachedFile {
                        stamp: head.stamp,
                        hash: head.hash,
                        offset: base + head.offset,
                        length: head.length,
                        summary: Arc::new(head.summary),
                    },
                )
            })
            .collect();
        Some(Cache {
            files,
            store: Some(store),
        })
    }

    /// Writes the cache next to its final place and renames it, so a reader never sees half of it.
    pub fn save(path: &Path, entries: Vec<Entry>) -> io::Result<Saved> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut offset = 0u64;
        let mut heads = Vec::with_capacity(entries.len());
        for entry in &entries {
            let length = u32::try_from(entry.payload.length()).map_err(io::Error::other)?;
            heads.push(StoredHead {
                path: entry.path.clone(),
                stamp: entry.stamp,
                hash: entry.hash,
                offset,
                length,
                summary: (*entry.summary).clone(),
            });
            offset += u64::from(length);
        }
        let header = postcard::to_stdvec(&Header {
            magic: MAGIC,
            schema: SCHEMA,
        })
        .map_err(io::Error::other)?;
        let index = postcard::to_stdvec(&heads).map_err(io::Error::other)?;
        let base = (header.len() + 8 + index.len()) as u64;
        let temporary = path.with_extension("tmp");
        {
            let mut out = io::BufWriter::new(fs::File::create(&temporary)?);
            out.write_all(&header)?;
            out.write_all(&(index.len() as u64).to_le_bytes())?;
            out.write_all(&index)?;
            for entry in &entries {
                match &entry.payload {
                    Payload::Bytes(bytes) => out.write_all(bytes)?,
                    Payload::Stored { store, offset, length } => {
                        out.write_all(&store.read(*offset, *length as usize)?)?;
                    }
                }
            }
            out.flush()?;
        }
        fs::rename(&temporary, path)?;
        let places = heads
            .iter()
            .map(|head| (head.path.clone(), (base + head.offset, head.length)))
            .collect();
        Ok(Saved {
            store: Arc::new(SymbolStore::open(path)?),
            places,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ExtractOptions, extract};
    use php_syntax::parse;

    #[test]
    fn round_trips_records_and_drops_a_foreign_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let source = dir.path().join("a.php");
        fs::write(&source, "<?php class A { public function f(int $x): ?A {} }").expect("written");
        let text = fs::read_to_string(&source).expect("read");
        let symbols = extract(&parse(&text).syntax(), ExtractOptions::default());
        let stamp = Stamp::of(&source).expect("stat");
        let summary = Arc::new(FileSummary::of(&symbols));
        let entry =
            Entry::new(&source, stamp, content_hash(text.as_bytes()), summary.clone(), &symbols).expect("serialized");

        let path = dir.path().join("cache/project.bin");
        let saved = Cache::save(&path, vec![entry]).expect("saved");
        let key = source.to_string_lossy().into_owned();
        assert_eq!(
            saved.places[&key].1 as usize,
            postcard::to_stdvec(&symbols).expect("bytes").len()
        );
        let cache = Cache::load(&path);
        let loaded = cache.files.get(&key).expect("a record");
        assert_eq!(loaded.stamp, stamp);
        assert_eq!(*loaded.summary, *summary);
        let store = cache.store.as_ref().expect("a store");
        assert_eq!(*loaded.source(store).load().expect("decoded"), symbols);

        // Saving again with the stored payload copies it over.
        let again = Entry {
            path: key.clone(),
            stamp,
            hash: loaded.hash,
            summary,
            payload: Payload::Stored {
                store: store.clone(),
                offset: loaded.offset,
                length: loaded.length,
            },
        };
        let second = dir.path().join("cache/second.bin");
        Cache::save(&second, vec![again]).expect("saved");
        let copied = Cache::load(&second);
        let copy = copied.files.get(&key).expect("a record");
        assert_eq!(
            *copy
                .source(copied.store.as_ref().expect("a store"))
                .load()
                .expect("decoded"),
            symbols
        );

        fs::write(&path, b"garbage").expect("written");
        assert!(Cache::load(&path).files.is_empty());
        assert!(Cache::load(&dir.path().join("missing.bin")).files.is_empty());
    }
}
