//! The table of contents: what it holds, and its bytes on disk.

use std::collections::BTreeMap;

use super::cipher::{self, TOC_STREAM};
use super::{content_hash, PakError, PAK_VERSION, TOC_MAGIC};

pub(super) const TOC_HEADER_LEN: usize = 56;
pub(super) const CONTAINER_HEADER_LEN: u64 = 16;
/// Upper bound on a table of contents we load (a corrupt length must not
/// allocate gigabytes).
const MAX_TOC_LEN: u64 = 512 * 1024 * 1024;
/// Upper bound on one block (a corrupt entry must not allocate gigabytes).
pub(super) const MAX_BLOCK_LEN: u32 = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Codec {
    Raw,
    Zstd,
}

impl Codec {
    fn to_byte(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::Zstd => 1,
        }
    }

    fn from_byte(byte: u8) -> Result<Self, PakError> {
        match byte {
            0 => Ok(Self::Raw),
            1 => Ok(Self::Zstd),
            other => Err(PakError::Corrupt(format!("unknown codec {other}"))),
        }
    }
}

/// One stored block of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Block {
    /// Absolute offset in its container file.
    pub offset: u64,
    pub stored: u32,
    pub raw: u32,
    pub codec: Codec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TocEntry {
    pub len: u64,
    pub hash: u128,
    pub container: u32,
    /// In order; together they decode to the entry's bytes.
    pub blocks: Vec<Block>,
}

impl TocEntry {
    pub fn stored_len(&self) -> u64 {
        self.blocks.iter().map(|b| u64::from(b.stored)).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Toc {
    pub block_size: u32,
    /// Container index → its file's length.
    pub containers: BTreeMap<u32, u64>,
    pub entries: BTreeMap<String, TocEntry>,
}

impl Toc {
    /// The table of contents as a file: header, then the body compressed and
    /// encrypted with `key`.
    pub fn encode(&self, key: &[u8; 32]) -> Result<Vec<u8>, PakError> {
        let mut body = Vec::new();
        put_u32(&mut body, self.block_size);
        put_u32(&mut body, self.containers.len() as u32);
        for (index, len) in &self.containers {
            put_u32(&mut body, *index);
            put_u64(&mut body, *len);
        }
        put_u32(&mut body, self.entries.len() as u32);
        for (path, entry) in &self.entries {
            let path_len =
                u16::try_from(path.len()).map_err(|_| PakError::BadPath(path.clone()))?;
            body.extend_from_slice(&path_len.to_le_bytes());
            body.extend_from_slice(path.as_bytes());
            put_u64(&mut body, entry.len);
            body.extend_from_slice(&entry.hash.to_le_bytes());
            put_u32(&mut body, entry.container);
            put_u32(&mut body, entry.blocks.len() as u32);
            for block in &entry.blocks {
                put_u64(&mut body, block.offset);
                put_u32(&mut body, block.stored);
                put_u32(&mut body, block.raw);
                body.push(block.codec.to_byte());
            }
        }
        let body_hash = content_hash(&body);
        let raw_len = body.len() as u64;
        let mut stored =
            zstd::bulk::compress(&body, 3).map_err(|e| PakError::Codec(e.to_string()))?;
        cipher::apply(key, TOC_STREAM, &mut stored);

        let mut out = Vec::with_capacity(TOC_HEADER_LEN + stored.len());
        out.extend_from_slice(&TOC_MAGIC);
        put_u32(&mut out, PAK_VERSION);
        put_u32(&mut out, 0);
        put_u64(&mut out, stored.len() as u64);
        put_u64(&mut out, raw_len);
        out.extend_from_slice(&body_hash.to_le_bytes());
        out.extend_from_slice(&cipher::key_check(key));
        out.extend_from_slice(&stored);
        Ok(out)
    }

    /// Read a table of contents file with `key`.
    pub fn decode(file: &[u8], key: &[u8; 32]) -> Result<Self, PakError> {
        let header = file.get(..TOC_HEADER_LEN).ok_or(PakError::NotAPak)?;
        if header[..8] != TOC_MAGIC {
            return Err(PakError::NotAPak);
        }
        let mut at = Reader::new(&header[8..]);
        let version = at.u32()?;
        if version != PAK_VERSION {
            return Err(PakError::UnsupportedVersion(version));
        }
        let _flags = at.u32()?;
        let stored_len = at.u64()?;
        let raw_len = at.u64()?;
        let body_hash = at.u128()?;
        let check = at.take(8)?;
        if check != cipher::key_check(key) {
            return Err(PakError::WrongKey);
        }
        if raw_len > MAX_TOC_LEN || stored_len != (file.len() - TOC_HEADER_LEN) as u64 {
            return Err(PakError::Corrupt("table of contents length".into()));
        }
        let mut stored = file[TOC_HEADER_LEN..].to_vec();
        cipher::apply(key, TOC_STREAM, &mut stored);
        let body = zstd::bulk::decompress(&stored, raw_len as usize)
            .map_err(|e| PakError::Corrupt(format!("table of contents: {e}")))?;
        if body.len() as u64 != raw_len || content_hash(&body) != body_hash {
            return Err(PakError::Corrupt("table of contents hash".into()));
        }

        let mut at = Reader::new(&body);
        let block_size = at.u32()?;
        let mut toc = Toc {
            block_size,
            ..Toc::default()
        };
        for _ in 0..at.u32()? {
            let index = at.u32()?;
            let len = at.u64()?;
            if index >= cipher::MAX_CONTAINERS || len > cipher::MAX_CONTAINER_LEN {
                return Err(PakError::Corrupt(format!("container {index}")));
            }
            toc.containers.insert(index, len);
        }
        for _ in 0..at.u32()? {
            let path_len = usize::from(u16::from_le_bytes(at.array()?));
            let path = std::str::from_utf8(at.take(path_len)?)
                .map_err(|_| PakError::Corrupt("non-UTF-8 path".into()))?
                .to_owned();
            if crate::normalize_rel(&path).as_deref() != Some(path.as_str()) {
                return Err(PakError::BadPath(path));
            }
            let len = at.u64()?;
            let hash = at.u128()?;
            let container = at.u32()?;
            let container_len = *toc
                .containers
                .get(&container)
                .ok_or_else(|| PakError::Corrupt(format!("`{path}` names no container")))?;
            let count = at.u32()? as usize;
            let mut blocks = Vec::with_capacity(count.min(1 << 16));
            let mut decoded = 0u64;
            for _ in 0..count {
                let block = Block {
                    offset: at.u64()?,
                    stored: at.u32()?,
                    raw: at.u32()?,
                    codec: Codec::from_byte(at.u8()?)?,
                };
                let end = block.offset.checked_add(u64::from(block.stored));
                if block.offset < CONTAINER_HEADER_LEN
                    || end.is_none_or(|end| end > container_len)
                    || block.raw > MAX_BLOCK_LEN
                    || block.stored > MAX_BLOCK_LEN
                {
                    return Err(PakError::Corrupt(format!(
                        "`{path}` points outside its container"
                    )));
                }
                decoded += u64::from(block.raw);
                blocks.push(block);
            }
            if decoded != len {
                return Err(PakError::Corrupt(format!("`{path}` blocks do not add up")));
            }
            toc.entries.insert(
                path,
                TocEntry {
                    len,
                    hash,
                    container,
                    blocks,
                },
            );
        }
        if !at.is_empty() {
            return Err(PakError::Corrupt(
                "trailing bytes in table of contents".into(),
            ));
        }
        Ok(toc)
    }
}

/// The 16-byte header of container `index`.
pub(super) fn container_header(index: u32) -> [u8; CONTAINER_HEADER_LEN as usize] {
    let mut header = [0u8; CONTAINER_HEADER_LEN as usize];
    header[..8].copy_from_slice(&super::CONTAINER_MAGIC);
    header[8..12].copy_from_slice(&PAK_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&index.to_le_bytes());
    header
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], PakError> {
        let slice = self
            .bytes
            .get(self.at..self.at.checked_add(n).unwrap_or(usize::MAX))
            .ok_or_else(|| PakError::Corrupt("truncated table of contents".into()))?;
        self.at += n;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], PakError> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N)?);
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, PakError> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, PakError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, PakError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn u128(&mut self) -> Result<u128, PakError> {
        Ok(u128::from_le_bytes(self.array()?))
    }

    fn is_empty(&self) -> bool {
        self.at == self.bytes.len()
    }
}
