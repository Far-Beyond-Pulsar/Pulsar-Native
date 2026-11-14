//! Reading a pak set: the table of contents once, then entries by path,
//! touching only their own blocks. Containers are opened on first use.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use engine_fs::virtual_fs;
use parking_lot::Mutex;

use super::cipher;
use super::format::{Codec, Toc, TocEntry, CONTAINER_HEADER_LEN};
use super::{
    container_file_name, content_hash, ContentKey, PakEntry, PakError, CONTAINER_MAGIC,
    PAK_VERSION, TOC_FILE_NAME,
};

/// An open pak set.
pub struct PakSet {
    dir: PathBuf,
    key: [u8; 32],
    toc: Toc,
    /// The table of contents file's hash: identifies this set's contents.
    content_id: u128,
    containers: Mutex<HashMap<u32, Box<dyn engine_fs::ReadSeek>>>,
}

impl std::fmt::Debug for PakSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PakSet")
            .field("dir", &self.dir)
            .field("entries", &self.toc.entries.len())
            .field("containers", &self.toc.containers.len())
            .finish()
    }
}

fn io_error(error: anyhow::Error) -> PakError {
    PakError::Io(
        error
            .downcast::<std::io::Error>()
            .unwrap_or_else(|other| std::io::Error::other(other.to_string())),
    )
}

impl PakSet {
    /// Whether `dir` holds a pak set (its table of contents).
    pub fn exists_in(dir: &Path) -> bool {
        virtual_fs::is_file(&dir.join(TOC_FILE_NAME))
    }

    /// Open the pak set in `dir` with `key`.
    pub fn open(dir: impl AsRef<Path>, key: ContentKey) -> Result<Self, PakError> {
        let dir = dir.as_ref().to_path_buf();
        let file = virtual_fs::read_file(&dir.join(TOC_FILE_NAME)).map_err(io_error)?;
        let toc = Toc::decode(&file, &key.0)?;
        Ok(Self {
            dir,
            key: key.0,
            toc,
            content_id: content_hash(&file),
            containers: Mutex::new(HashMap::new()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// A hash identifying this set's contents (its table of contents holds
    /// every entry's hash).
    pub fn content_id(&self) -> u128 {
        self.content_id
    }

    /// Every entry, by content-relative path.
    pub fn entries(&self) -> BTreeMap<&str, PakEntry> {
        self.toc
            .entries
            .iter()
            .map(|(path, entry)| (path.as_str(), public(entry)))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.toc.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.toc.entries.is_empty()
    }

    /// The container files this set reads, by index.
    pub fn container_files(&self) -> Vec<String> {
        self.toc.containers.keys().map(|i| container_file_name(*i)).collect()
    }

    /// The entry for content-relative `path`.
    pub fn entry(&self, path: &str) -> Option<PakEntry> {
        self.toc.entries.get(&crate::normalize_rel(path)?).map(public)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entry(path).is_some()
    }

    /// Whether any entry lies under directory `dir` (content-relative).
    pub fn contains_dir(&self, dir: &str) -> bool {
        match crate::normalize_rel(dir) {
            None => !self.toc.entries.is_empty(),
            Some(dir) => {
                let prefix = format!("{dir}/");
                self.toc
                    .entries
                    .range(prefix.clone()..)
                    .next()
                    .is_some_and(|(p, _)| p.starts_with(&prefix))
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
        for (path, entry) in self.toc.entries.range(prefix.clone()..) {
            let Some(rest) = path.strip_prefix(&prefix) else {
                break;
            };
            match rest.split_once('/') {
                Some((child, _)) => {
                    out.entry(child.to_owned()).or_insert((true, 0));
                }
                None => {
                    out.insert(rest.to_owned(), (false, entry.len));
                }
            }
        }
        out.into_iter()
            .map(|(name, (is_dir, len))| (name, is_dir, len))
            .collect()
    }

    /// Read content-relative `path`: its blocks, decrypted and decompressed,
    /// checked against its hash.
    pub fn read(&self, path: &str) -> Result<Vec<u8>, PakError> {
        let rel = crate::normalize_rel(path).ok_or_else(|| PakError::BadPath(path.to_owned()))?;
        let entry = self
            .toc
            .entries
            .get(&rel)
            .ok_or_else(|| PakError::Missing(path.to_owned()))?;
        let mut out = Vec::with_capacity(entry.len as usize);
        let mut stored = Vec::new();
        {
            let mut containers = self.containers.lock();
            let file = match containers.entry(entry.container) {
                std::collections::hash_map::Entry::Occupied(open) => open.into_mut(),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(self.open_container(entry.container)?)
                }
            };
            for block in &entry.blocks {
                stored.resize(block.stored as usize, 0);
                file.seek(SeekFrom::Start(block.offset))?;
                file.read_exact(&mut stored)?;
                cipher::apply(&self.key, cipher::block_stream(entry.container, block.offset), &mut stored);
                match block.codec {
                    Codec::Raw => out.extend_from_slice(&stored),
                    Codec::Zstd => {
                        let raw = zstd::bulk::decompress(&stored, block.raw as usize)
                            .map_err(|_| PakError::HashMismatch(rel.clone()))?;
                        out.extend_from_slice(&raw);
                    }
                }
            }
        }
        if out.len() as u64 != entry.len || content_hash(&out) != entry.hash {
            return Err(PakError::HashMismatch(rel));
        }
        Ok(out)
    }

    fn open_container(&self, index: u32) -> Result<Box<dyn engine_fs::ReadSeek>, PakError> {
        let path = self.dir.join(container_file_name(index));
        let mut file = virtual_fs::open_read(&path).map_err(io_error)?;
        let len = file.seek(SeekFrom::End(0))?;
        file.seek(SeekFrom::Start(0))?;
        let mut header = [0u8; CONTAINER_HEADER_LEN as usize];
        file.read_exact(&mut header).map_err(|_| PakError::NotAPak)?;
        let version = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
        let own_index = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
        if header[..8] != CONTAINER_MAGIC
            || version != PAK_VERSION
            || own_index != index
            || Some(&len) != self.toc.containers.get(&index)
        {
            return Err(PakError::Corrupt(format!(
                "{} is not the container the table of contents names",
                path.display()
            )));
        }
        Ok(file)
    }
}

fn public(entry: &TocEntry) -> PakEntry {
    PakEntry {
        len: entry.len,
        hash: entry.hash,
    }
}
