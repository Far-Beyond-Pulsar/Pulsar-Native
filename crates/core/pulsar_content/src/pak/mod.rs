//! The pak set: a packaged game's content as a table of contents and a set of
//! size-bounded containers.
//!
//! ```text
//! Content/
//!   game.toc          the table of contents (compressed, encrypted)
//!   game_0000.pak     containers: compressed, encrypted blocks, about
//!   game_0001.pak     `PackSettings::container_size` each
//!   ...
//! ```
//!
//! # What is in each file (version 2, little-endian)
//!
//! **Container** `game_NNNN.pak`: a 16-byte header (`PULSARPC`, version
//! `u32`, its index `u32`), then blocks back to back. A container holds no
//! structure of its own: the table of contents says where every block is.
//!
//! **Table of contents** `game.toc`: a 56-byte plaintext header (`PULSARTC`,
//! version, flags, the body's stored and raw length, the body's XXH3-128, and
//! an 8-byte key check), then the body, zstd-compressed and encrypted. The
//! body lists the containers (index, length) and every entry: its
//! content-relative path, length, XXH3-128, container, and its blocks
//! (offset, stored length, raw length, codec).
//!
//! # Blocks
//!
//! An entry's bytes are cut into blocks of `PackSettings::block_size` (the
//! last one shorter), each compressed with zstd (stored raw when that is not
//! smaller) and encrypted with ChaCha20 under the project's [`ContentKey`].
//! Each block has its own keystream (the stream number is the block's
//! container and offset), so any block decodes alone: a read touches only
//! the blocks of the entry it reads, and a container's blocks need no order.
//! An entry never spans two containers; one larger than the container size
//! has a container to itself (the sizes are "fixed-ish").
//!
//! # Names
//!
//! No path is stored in plaintext anywhere: they are inside the encrypted
//! table of contents. Container file names say nothing about content.
//!
//! # Updating a pak set
//!
//! [`write_pak_set`] reuses what it can. An entry whose bytes did not change
//! keeps its blocks where they are; changed and new entries go to new
//! containers numbered after every existing one; the new table of contents
//! replaces the old one atomically, and only then are containers nothing
//! points at deleted. When too much of the set is dead bytes (entries that
//! were replaced), the whole set is written again instead. Container
//! numbers only grow, so a new container never overwrites one the current
//! table of contents points at, and an interrupted write leaves a working
//! set (the old table of contents and its containers).
//!
//! The key is not a secret from a determined user: the game holds it to read
//! its own content. The encryption keeps content from being browsed, copied
//! out or edited in place; [`ContentKey`] says where the key lives.

mod cipher;
mod format;
mod key;
mod reader;
mod writer;

pub use key::{installed_content_key, install_content_key, ContentKey, CONTENT_KEY_FILE};
pub use reader::PakSet;
pub use writer::{write_pak_set, PackEvent, PackReport, PackSettings};

use std::io;

/// The table of contents' file name inside `Content/`.
pub const TOC_FILE_NAME: &str = "game.toc";
/// A container's file name: `game_0000.pak`, `game_0001.pak`, ...
pub fn container_file_name(index: u32) -> String {
    format!("game_{index:04}.pak")
}
/// The container index in a container file name, if it is one.
pub fn container_index(file_name: &str) -> Option<u32> {
    let digits = file_name.strip_prefix("game_")?.strip_suffix(".pak")?;
    if digits.len() < 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let index: u32 = digits.parse().ok()?;
    (container_file_name(index) == file_name).then_some(index)
}

/// The format version this crate reads and writes.
pub const PAK_VERSION: u32 = 2;
pub const TOC_MAGIC: [u8; 8] = *b"PULSARTC";
pub const CONTAINER_MAGIC: [u8; 8] = *b"PULSARPC";

/// One file in a pak set, as readers see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PakEntry {
    /// Its length in bytes, decoded.
    pub len: u64,
    /// XXH3-128 of its decoded bytes.
    pub hash: u128,
}

#[derive(Debug, thiserror::Error)]
pub enum PakError {
    #[error("pak I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("not a Pulsar pak set")]
    NotAPak,
    #[error("pak version {0} is not supported (this engine reads {PAK_VERSION})")]
    UnsupportedVersion(u32),
    #[error("the content key does not open this pak set (content packed for another build)")]
    WrongKey,
    #[error("corrupt pak: {0}")]
    Corrupt(String),
    #[error("`{0}` is not in the pak")]
    Missing(String),
    #[error("`{0}` in the pak does not match its hash (corrupt or modified)")]
    HashMismatch(String),
    #[error("`{0}` is not a valid content-relative path")]
    BadPath(String),
    #[error("compression: {0}")]
    Codec(String),
}

impl From<PakError> for io::Error {
    fn from(error: PakError) -> Self {
        match error {
            PakError::Io(error) => error,
            PakError::Missing(_) => io::Error::new(io::ErrorKind::NotFound, error.to_string()),
            other => io::Error::new(io::ErrorKind::InvalidData, other.to_string()),
        }
    }
}

/// XXH3-128 of `bytes`, the hash pak sets and the asset registry use.
pub fn content_hash(bytes: &[u8]) -> u128 {
    twox_hash::XxHash3_128::oneshot(bytes)
}

#[cfg(test)]
mod tests;
