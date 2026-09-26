//! `game.pak`: one indexed archive holding a packaged game's content.
//!
//! # Format (version 1, little-endian)
//!
//! ```text
//! offset 0   header (40 bytes)
//!              magic        [u8; 8]  "PULSARPK"
//!              version      u32      PAK_VERSION
//!              flags        u32      0 (reserved)
//!              entry_count  u32
//!              reserved     u32      0
//!              toc_offset   u64      where the table of contents starts
//!              toc_len      u64      its length in bytes
//! offset 40  blobs, back to back, uncompressed, in TOC order
//! toc_offset table of contents, one record per entry, sorted by path:
//!              path_len     u16
//!              path         [u8; path_len]  UTF-8, content-relative, '/'-separated
//!              offset       u64      absolute offset of the blob
//!              len          u64
//!              hash         u128     XXH3-128 of the blob
//! ```
//!
//! The table of contents comes last so a writer streams blobs without
//! knowing them all up front. Readers load the TOC once and read blobs by
//! offset; every read checks the blob's hash. Compression and encryption
//! would be `flags` bits in a later version (the settings schema already
//! names them); version 1 has neither.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;

/// The archive's file name inside `Content/`.
pub const PAK_FILE_NAME: &str = "game.pak";
/// First bytes of every pak.
pub const PAK_MAGIC: [u8; 8] = *b"PULSARPK";
/// The format version this crate reads and writes.
pub const PAK_VERSION: u32 = 1;
const HEADER_LEN: u64 = 40;
/// Upper bound on a TOC we are willing to load (a corrupt length must not
/// allocate gigabytes).
const MAX_TOC_LEN: u64 = 256 * 1024 * 1024;

/// Where one file lives in a pak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PakEntry {
    pub offset: u64,
    pub len: u64,
    /// XXH3-128 of the contents.
    pub hash: u128,
}

#[derive(Debug, thiserror::Error)]
pub enum PakError {
    #[error("pak I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("not a Pulsar pak file")]
    NotAPak,
    #[error("pak version {0} is not supported (this engine reads {PAK_VERSION})")]
    UnsupportedVersion(u32),
    #[error("corrupt pak: {0}")]
    Corrupt(String),
    #[error("`{0}` is not in the pak")]
    Missing(String),
    #[error("`{0}` in the pak does not match its hash (corrupt or modified)")]
    HashMismatch(String),
    #[error("`{0}` is not a valid content-relative path")]
    BadPath(String),
    #[error("`{0}` was added to the pak twice")]
    Duplicate(String),
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

/// XXH3-128 of `bytes`, the hash paks and the asset registry use.
pub fn content_hash(bytes: &[u8]) -> u128 {
    twox_hash::XxHash3_128::oneshot(bytes)
}

/// Writes a pak: [`add`](Self::add) files, then [`finish`](Self::finish).
pub struct PakWriter {
    out: BufWriter<File>,
    path: PathBuf,
    offset: u64,
    entries: BTreeMap<String, PakEntry>,
}

impl PakWriter {
    /// Start a pak at `path` (truncating any file there).
    pub fn create(path: impl AsRef<Path>) -> Result<Self, PakError> {
        let path = path.as_ref().to_path_buf();
        let mut out = BufWriter::new(File::create(&path)?);
        // Placeholder header, rewritten by `finish`.
        out.write_all(&[0u8; HEADER_LEN as usize])?;
        Ok(Self { out, path, offset: HEADER_LEN, entries: BTreeMap::new() })
    }

    /// Add `bytes` as content-relative `path`. Returns its entry.
    pub fn add(&mut self, path: &str, bytes: &[u8]) -> Result<PakEntry, PakError> {
        let path = crate::normalize_rel(path).ok_or_else(|| PakError::BadPath(path.to_owned()))?;
        if path.len() > usize::from(u16::MAX) {
            return Err(PakError::BadPath(path));
        }
        if self.entries.contains_key(&path) {
            return Err(PakError::Duplicate(path));
        }
        self.out.write_all(bytes)?;
        let entry = PakEntry { offset: self.offset, len: bytes.len() as u64, hash: content_hash(bytes) };
        self.offset += entry.len;
        self.entries.insert(path, entry);
        Ok(entry)
    }

    /// Whether `path` was added.
    pub fn contains(&self, path: &str) -> bool {
        crate::normalize_rel(path).is_some_and(|p| self.entries.contains_key(&p))
    }

    /// Write the table of contents and the header. Returns the entries.
    pub fn finish(mut self) -> Result<BTreeMap<String, PakEntry>, PakError> {
        let toc_offset = self.offset;
        let mut toc = Vec::new();
        for (path, entry) in &self.entries {
            toc.extend_from_slice(&(path.len() as u16).to_le_bytes());
            toc.extend_from_slice(path.as_bytes());
            toc.extend_from_slice(&entry.offset.to_le_bytes());
            toc.extend_from_slice(&entry.len.to_le_bytes());
            toc.extend_from_slice(&entry.hash.to_le_bytes());
        }
        self.out.write_all(&toc)?;
        let mut header = Vec::with_capacity(HEADER_LEN as usize);
        header.extend_from_slice(&PAK_MAGIC);
        header.extend_from_slice(&PAK_VERSION.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        header.extend_from_slice(&toc_offset.to_le_bytes());
        header.extend_from_slice(&(toc.len() as u64).to_le_bytes());
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(&header)?;
        self.out.flush()?;
        tracing::debug!(path = %self.path.display(), entries = self.entries.len(), "pak written");
        Ok(self.entries)
    }
}

/// An open pak: its table of contents, and reads by path.
pub struct PakReader {
    path: PathBuf,
    file: Mutex<File>,
    entries: BTreeMap<String, PakEntry>,
    /// Hash of the table of contents: identifies this pak's contents.
    toc_hash: u128,
}

impl std::fmt::Debug for PakReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PakReader").field("path", &self.path).field("entries", &self.entries.len()).finish()
    }
}

impl PakReader {
    /// Open the pak at `path` and load its table of contents.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, PakError> {
        let path = path.as_ref().to_path_buf();
        let mut file = File::open(&path)?;
        let file_len = file.metadata()?.len();
        let mut header = [0u8; HEADER_LEN as usize];
        file.read_exact(&mut header).map_err(|_| PakError::NotAPak)?;
        if header[..8] != PAK_MAGIC {
            return Err(PakError::NotAPak);
        }
        let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap_or_default());
        let u64_at = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap_or_default());
        let version = u32_at(8);
        if version != PAK_VERSION {
            return Err(PakError::UnsupportedVersion(version));
        }
        let count = u32_at(16) as usize;
        let (toc_offset, toc_len) = (u64_at(24), u64_at(32));
        if toc_len > MAX_TOC_LEN || toc_offset.checked_add(toc_len) != Some(file_len) || toc_offset < HEADER_LEN {
            return Err(PakError::Corrupt("table of contents out of range".into()));
        }
        file.seek(SeekFrom::Start(toc_offset))?;
        let mut toc = vec![0u8; toc_len as usize];
        file.read_exact(&mut toc)?;
        let toc_hash = content_hash(&toc);

        let mut entries = BTreeMap::new();
        let mut at = 0usize;
        let take = |at: &mut usize, n: usize| -> Result<&[u8], PakError> {
            let slice = toc.get(*at..*at + n).ok_or_else(|| PakError::Corrupt("truncated table of contents".into()))?;
            *at += n;
            Ok(slice)
        };
        for _ in 0..count {
            let path_len = u16::from_le_bytes(take(&mut at, 2)?.try_into().unwrap_or_default()) as usize;
            let path = std::str::from_utf8(take(&mut at, path_len)?)
                .map_err(|_| PakError::Corrupt("non-UTF-8 path".into()))?
                .to_owned();
            let offset = u64::from_le_bytes(take(&mut at, 8)?.try_into().unwrap_or_default());
            let len = u64::from_le_bytes(take(&mut at, 8)?.try_into().unwrap_or_default());
            let hash = u128::from_le_bytes(take(&mut at, 16)?.try_into().unwrap_or_default());
            if offset < HEADER_LEN || offset.checked_add(len).is_none_or(|end| end > toc_offset) {
                return Err(PakError::Corrupt(format!("`{path}` points outside the blob area")));
            }
            if crate::normalize_rel(&path).as_deref() != Some(path.as_str()) {
                return Err(PakError::BadPath(path));
            }
            entries.insert(path, PakEntry { offset, len, hash });
        }
        if at != toc.len() {
            return Err(PakError::Corrupt("trailing bytes in table of contents".into()));
        }
        Ok(Self { path, file: Mutex::new(file), entries, toc_hash })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every entry, by content-relative path.
    pub fn entries(&self) -> &BTreeMap<String, PakEntry> {
        &self.entries
    }

    /// A hash identifying this pak's contents (of its table of contents,
    /// which holds every blob's hash).
    pub fn content_id(&self) -> u128 {
        self.toc_hash
    }

    /// The entry for content-relative `path`.
    pub fn entry(&self, path: &str) -> Option<&PakEntry> {
        self.entries.get(&crate::normalize_rel(path)?)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entry(path).is_some()
    }

    /// Whether any entry lies under directory `dir` (content-relative).
    pub fn contains_dir(&self, dir: &str) -> bool {
        match crate::normalize_rel(dir) {
            None => !self.entries.is_empty(),
            Some(dir) => {
                let prefix = format!("{dir}/");
                self.entries.range(prefix.clone()..).next().is_some_and(|(p, _)| p.starts_with(&prefix))
            }
        }
    }

    /// Immediate children of directory `dir` ("" for the root), as
    /// `(name, is_dir, size)`.
    pub fn list_dir(&self, dir: &str) -> Vec<(String, bool, u64)> {
        let prefix = match crate::normalize_rel(dir) {
            Some(dir) => format!("{dir}/"),
            None => String::new(),
        };
        let mut out: BTreeMap<String, (bool, u64)> = BTreeMap::new();
        for (path, entry) in self.entries.range(prefix.clone()..) {
            let Some(rest) = path.strip_prefix(&prefix) else { break };
            match rest.split_once('/') {
                Some((child, _)) => {
                    out.entry(child.to_owned()).or_insert((true, 0));
                }
                None => {
                    out.insert(rest.to_owned(), (false, entry.len));
                }
            }
        }
        out.into_iter().map(|(name, (is_dir, len))| (name, is_dir, len)).collect()
    }

    /// Read content-relative `path`, checking its hash.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, PakError> {
        let entry = *self.entry(path).ok_or_else(|| PakError::Missing(path.to_owned()))?;
        let mut bytes = vec![0u8; entry.len as usize];
        {
            let mut file = self.file.lock();
            file.seek(SeekFrom::Start(entry.offset))?;
            file.read_exact(&mut bytes)?;
        }
        if content_hash(&bytes) != entry.hash {
            return Err(PakError::HashMismatch(path.to_owned()));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_every_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PAK_FILE_NAME);
        let mut writer = PakWriter::create(&path).unwrap();
        writer.add("Pulsar/project.json", br#"{"name":"x"}"#).unwrap();
        writer.add("assets\\meshes/a.mesh", &[1, 2, 3]).unwrap();
        writer.add("empty.txt", b"").unwrap();
        assert!(matches!(writer.add("./Pulsar/project.json", b"again"), Err(PakError::Duplicate(_))));
        assert!(matches!(writer.add("../escape", b""), Err(PakError::BadPath(_))));
        let written = writer.finish().unwrap();
        assert_eq!(written.len(), 3);

        let pak = PakReader::open(&path).unwrap();
        assert_eq!(pak.entries(), &written);
        assert_eq!(pak.read("Pulsar/project.json").unwrap(), br#"{"name":"x"}"#);
        assert_eq!(pak.read("assets/meshes/a.mesh").unwrap(), [1, 2, 3]);
        assert_eq!(pak.read("empty.txt").unwrap(), b"");
        assert!(matches!(pak.read("nope"), Err(PakError::Missing(_))));
        assert!(pak.contains_dir("assets"));
        assert!(pak.contains_dir("assets/meshes"));
        assert!(!pak.contains_dir("asset"));
        assert_eq!(
            pak.list_dir(""),
            vec![("Pulsar".into(), true, 0), ("assets".into(), true, 0), ("empty.txt".into(), false, 0)]
        );
        assert_eq!(pak.list_dir("assets/meshes"), vec![("a.mesh".into(), false, 3)]);
    }

    #[test]
    fn corruption_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PAK_FILE_NAME);
        let mut writer = PakWriter::create(&path).unwrap();
        writer.add("a", b"hello").unwrap();
        writer.finish().unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[HEADER_LEN as usize] ^= 0xff; // flip a blob byte
        std::fs::write(&path, &bytes).unwrap();
        let pak = PakReader::open(&path).unwrap();
        assert!(matches!(pak.read("a"), Err(PakError::HashMismatch(_))));

        std::fs::write(&path, b"not a pak at all, clearly").unwrap();
        assert!(matches!(PakReader::open(&path), Err(PakError::NotAPak)));
        bytes[8] = 9; // version
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(PakReader::open(&path), Err(PakError::UnsupportedVersion(9))));
        bytes[8] = 1;
        bytes.truncate(bytes.len() - 1);
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(PakReader::open(&path), Err(PakError::Corrupt(_))));
    }
}
