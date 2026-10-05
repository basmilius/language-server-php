//! The declarations of a file, where they are kept: in memory, or in the cache file until something
//! asks for them.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::model::FileSymbols;

/// An open cache file that declarations are read from. The file may be replaced on disk while this
/// is open; what was opened stays readable.
#[derive(Debug)]
pub struct SymbolStore {
    file: File,
}

impl SymbolStore {
    pub fn open(path: &Path) -> io::Result<SymbolStore> {
        Ok(SymbolStore {
            file: File::open(path)?,
        })
    }

    pub fn read(&self, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; length];
        read_exact_at(&self.file, &mut bytes, offset)?;
        Ok(bytes)
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, bytes, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !bytes.is_empty() {
        let read = std::os::windows::fs::FileExt::seek_read(file, bytes, offset)?;
        if read == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        bytes = &mut bytes[read..];
        offset += read as u64;
    }
    Ok(())
}

/// Where the declarations of a file can be had from.
#[derive(Clone, Debug)]
pub enum SymbolSource {
    Memory(Arc<FileSymbols>),
    /// A stretch of the cache file.
    Cached {
        store: Arc<SymbolStore>,
        offset: u64,
        length: u32,
    },
    /// The file itself, read and extracted again when the declarations are asked for. This is what
    /// a file is until the cache file with its declarations is written.
    Parse {
        path: PathBuf,
        stub: bool,
    },
}

impl SymbolSource {
    /// The declarations, read from the cache file when they are not in memory. `None` when the
    /// stretch cannot be read or does not decode.
    pub fn load(&self) -> Option<Arc<FileSymbols>> {
        match self {
            SymbolSource::Memory(symbols) => Some(symbols.clone()),
            SymbolSource::Cached { store, offset, length } => {
                let bytes = store.read(*offset, *length as usize).ok()?;
                postcard::from_bytes::<FileSymbols>(&bytes).ok().map(Arc::new)
            }
            SymbolSource::Parse { path, stub } => {
                let bytes = std::fs::read(path).ok()?;
                Some(Arc::new(crate::indexer::extract_text(
                    &String::from_utf8_lossy(&bytes),
                    *stub,
                )))
            }
        }
    }

    /// Whether the declarations are in the cache file.
    pub fn is_cached(&self) -> bool {
        matches!(self, SymbolSource::Cached { .. })
    }

    /// Whether the declarations can be let go and read again.
    pub fn is_lazy(&self) -> bool {
        !matches!(self, SymbolSource::Memory(_))
    }
}
