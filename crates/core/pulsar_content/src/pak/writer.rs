//! Writing a pak set, reusing the previous one where it can (see the module
//! doc of [`super`]).
//!
//! The pak set is a build output on the real machine (the folder next to
//! the game executable), written as files that must be seekable and renamed
//! atomically: every `std::fs` call of the pak code is in this file
//! (`architecture/std-fs-allowed.txt`).
#![allow(clippy::disallowed_methods)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use rayon::prelude::*;

use super::cipher::{self, MAX_CONTAINERS};
use super::format::{container_header, Block, Codec, Toc, TocEntry, CONTAINER_HEADER_LEN, MAX_BLOCK_LEN};
use super::{container_file_name, container_index, content_hash, ContentKey, PakError, TOC_FILE_NAME};

/// The single-file pak of format 1, replaced by a pak set.
const LEGACY_PAK_FILE: &str = "game.pak";

/// How a pak set is laid out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PackSettings {
    /// A container is closed once the next entry would take it past this
    /// (an entry larger than this has a container to itself).
    pub container_size: u64,
    /// Entries are cut into blocks of this many bytes, each compressed and
    /// encrypted alone: the unit a read decodes.
    pub block_size: u32,
    /// zstd level (1 fast .. 19 small).
    pub compression_level: i32,
    /// Above this share of dead bytes (entries replaced since the set was
    /// last written whole), the set is written whole again.
    pub max_dead_ratio: f64,
}

impl PackSettings {
    /// Development builds: fast compression.
    pub const DEV: Self = Self {
        container_size: 256 * 1024 * 1024,
        block_size: 128 * 1024,
        compression_level: 3,
        max_dead_ratio: 0.35,
    };
    /// Shipping builds: smaller, slower to pack.
    pub const SHIPPING: Self = Self {
        compression_level: 15,
        max_dead_ratio: 0.0,
        ..Self::DEV
    };
}

impl Default for PackSettings {
    fn default() -> Self {
        Self::DEV
    }
}

/// What [`write_pak_set`] reports as it goes.
#[derive(Clone, Debug, PartialEq)]
pub enum PackEvent {
    /// What will be written: `changed` entries packed, `reused` kept where
    /// they are; `rewrite` says why the whole set is written, if it is.
    Planned {
        entries: usize,
        reused: usize,
        changed: usize,
        changed_bytes: u64,
        rewrite: Option<String>,
    },
    /// Bytes of changed entries packed so far, of `total`.
    Packed { done: u64, total: u64 },
    ContainerWritten { file: String, bytes: u64 },
    Removed { file: String },
}

/// What [`write_pak_set`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PackReport {
    pub entries: usize,
    pub reused: usize,
    pub written: usize,
    /// Whether the whole set was written (no previous set, another key or
    /// layout, or too many dead bytes).
    pub rewritten: bool,
    /// Bytes of the entries written, before and after compression.
    pub written_raw: u64,
    pub written_stored: u64,
    /// The set's containers, and their total size.
    pub containers: Vec<String>,
    pub total_stored: u64,
    pub removed: Vec<String>,
}

/// Write `files` (content-relative path → bytes) as the pak set in `dir`,
/// encrypted with `key`. `cancelled` is polled between batches; a cancelled
/// write leaves the previous set as it was.
pub fn write_pak_set(
    dir: &Path,
    files: &BTreeMap<String, Vec<u8>>,
    key: ContentKey,
    settings: PackSettings,
    on_event: &mut dyn FnMut(PackEvent),
    cancelled: &dyn Fn() -> bool,
) -> Result<PackReport, PakError> {
    if settings.block_size == 0 || settings.block_size > MAX_BLOCK_LEN {
        return Err(PakError::Corrupt(format!("block size {}", settings.block_size)));
    }
    std::fs::create_dir_all(dir)?;

    let mut inputs: BTreeMap<String, &[u8]> = BTreeMap::new();
    for (path, bytes) in files {
        let rel = crate::normalize_rel(path).ok_or_else(|| PakError::BadPath(path.clone()))?;
        if rel.len() > usize::from(u16::MAX) || inputs.insert(rel, bytes).is_some() {
            return Err(PakError::BadPath(path.clone()));
        }
    }
    let hashes: BTreeMap<&String, u128> = inputs
        .par_iter()
        .map(|(path, bytes)| (path, content_hash(bytes)))
        .collect();

    // The previous set, if it can be built on.
    let (previous, mut rewrite) = match previous_set(dir, &key, settings.block_size) {
        Ok(toc) => (Some(toc), None),
        Err(reason) => (None, Some(reason)),
    };
    let mut reused: BTreeMap<String, TocEntry> = BTreeMap::new();
    if let Some(previous) = &previous {
        for (path, bytes) in &inputs {
            if let Some(entry) = previous.entries.get(path) {
                if entry.hash == hashes[path] && entry.len == bytes.len() as u64 {
                    reused.insert(path.clone(), entry.clone());
                }
            }
        }
        let total: u64 = previous
            .containers
            .values()
            .map(|len| len - CONTAINER_HEADER_LEN)
            .sum();
        let live: u64 = reused.values().map(TocEntry::stored_len).sum();
        let changed_raw: u64 = inputs
            .iter()
            .filter(|(path, _)| !reused.contains_key(*path))
            .map(|(_, bytes)| bytes.len() as u64)
            .sum();
        // The changed entries' stored size, estimated at the set's own
        // compression ratio: the dead share is of stored bytes throughout.
        let previous_raw: u64 = previous.entries.values().map(|e| e.len).sum();
        let ratio = total as f64 / previous_raw.max(1) as f64;
        let after = total as f64 + changed_raw as f64 * ratio;
        let dead_ratio = (total - live) as f64 / after.max(1.0);
        if dead_ratio > settings.max_dead_ratio {
            rewrite = Some(format!(
                "{:.0}% of the set would be replaced content (rewritten above {:.0}%)",
                dead_ratio * 100.0,
                settings.max_dead_ratio * 100.0
            ));
        }
    }
    if rewrite.is_some() {
        reused.clear();
    }

    let changed: Vec<(&String, &[u8])> = inputs
        .iter()
        .filter(|(path, _)| !reused.contains_key(*path))
        .map(|(path, bytes)| (path, *bytes))
        .collect();
    let changed_bytes: u64 = changed.iter().map(|(_, b)| b.len() as u64).sum();
    on_event(PackEvent::Planned {
        entries: inputs.len(),
        reused: reused.len(),
        changed: changed.len(),
        changed_bytes,
        rewrite: rewrite.clone(),
    });

    // New containers are numbered after every container there is, so none
    // overwrites one the current table of contents names.
    let existing = container_files(dir)?;
    let mut next_index = existing
        .keys()
        .copied()
        .chain(previous.iter().flat_map(|p| p.containers.keys().copied()))
        .max()
        .map_or(0, |max| max + 1);

    let mut toc = Toc {
        block_size: settings.block_size,
        containers: BTreeMap::new(),
        entries: reused,
    };
    if let Some(previous) = &previous {
        let still_used: BTreeSet<u32> = toc.entries.values().map(|e| e.container).collect();
        for index in still_used {
            toc.containers.insert(index, previous.containers[&index]);
        }
    }

    let mut report = PackReport {
        entries: inputs.len(),
        reused: toc.entries.len(),
        rewritten: rewrite.is_some(),
        ..PackReport::default()
    };

    // Pack the changed entries in batches: compress a batch in parallel,
    // then encrypt and write it, so memory holds one batch.
    const BATCH_BYTES: u64 = 64 * 1024 * 1024;
    let mut current: Option<ContainerWriter> = None;
    let mut written_containers: Vec<u32> = Vec::new();
    let mut done = 0u64;
    let mut start = 0;
    let result = (|| -> Result<(), PakError> {
        while start < changed.len() {
            if cancelled() {
                return Err(PakError::Io(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "packing was cancelled",
                )));
            }
            let mut end = start;
            let mut batch_bytes = 0u64;
            while end < changed.len() && (end == start || batch_bytes < BATCH_BYTES) {
                batch_bytes += changed[end].1.len() as u64;
                end += 1;
            }
            let compressed: Vec<Result<Vec<(Vec<u8>, u32, Codec)>, PakError>> = changed[start..end]
                .par_iter()
                .map(|(_, bytes)| {
                    bytes
                        .par_chunks(settings.block_size as usize)
                        .map(|raw| compress(raw, settings.compression_level))
                        .collect()
                })
                .collect();
            for ((path, bytes), blocks) in changed[start..end].iter().zip(compressed) {
                let blocks = blocks?;
                let stored: u64 = blocks.iter().map(|(b, _, _)| b.len() as u64).sum();
                let full = current
                    .as_ref()
                    .is_some_and(|c| c.len > CONTAINER_HEADER_LEN && c.len + stored > settings.container_size);
                if current.is_none() || full {
                    if let Some(done) = current.take() {
                        finish_container(done, &mut toc, &mut written_containers, on_event)?;
                    }
                    if next_index >= MAX_CONTAINERS {
                        return Err(PakError::Corrupt("too many containers".into()));
                    }
                    current = Some(ContainerWriter::create(dir, next_index)?);
                    next_index += 1;
                }
                let container = current.as_mut().ok_or_else(|| PakError::Corrupt("no container".into()))?;
                let mut entry_blocks = Vec::with_capacity(blocks.len());
                for (mut data, raw, codec) in blocks {
                    let offset = container.len;
                    cipher::apply(&key.0, cipher::block_stream(container.index, offset), &mut data);
                    container.write(&data)?;
                    entry_blocks.push(Block {
                        offset,
                        stored: data.len() as u32,
                        raw,
                        codec,
                    });
                }
                toc.entries.insert(
                    (*path).clone(),
                    TocEntry {
                        len: bytes.len() as u64,
                        hash: hashes[path],
                        container: container.index,
                        blocks: entry_blocks,
                    },
                );
                report.written += 1;
                report.written_raw += bytes.len() as u64;
                report.written_stored += stored;
            }
            done += batch_bytes;
            on_event(PackEvent::Packed {
                done,
                total: changed_bytes,
            });
            start = end;
        }
        if let Some(done) = current.take() {
            finish_container(done, &mut toc, &mut written_containers, on_event)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Nothing points at the new containers yet: remove them.
        if let Some(open) = current.take() {
            written_containers.push(open.index);
        }
        for index in written_containers {
            let _ = std::fs::remove_file(dir.join(container_file_name(index)));
        }
        return Err(error);
    }

    // The new table of contents replaces the old in one rename.
    let toc_bytes = toc.encode(&key.0)?;
    let temp = dir.join(format!("{TOC_FILE_NAME}.tmp"));
    std::fs::write(&temp, &toc_bytes)?;
    std::fs::rename(&temp, dir.join(TOC_FILE_NAME))?;

    // Only now: what nothing points at goes.
    for (index, name) in container_files(dir)? {
        if !toc.containers.contains_key(&index) {
            std::fs::remove_file(dir.join(&name))?;
            on_event(PackEvent::Removed { file: name.clone() });
            report.removed.push(name);
        }
    }
    let legacy = dir.join(LEGACY_PAK_FILE);
    if legacy.is_file() {
        std::fs::remove_file(&legacy)?;
        on_event(PackEvent::Removed {
            file: LEGACY_PAK_FILE.into(),
        });
        report.removed.push(LEGACY_PAK_FILE.into());
    }

    report.containers = toc.containers.keys().map(|i| container_file_name(*i)).collect();
    report.total_stored = toc.containers.values().map(|len| len - CONTAINER_HEADER_LEN).sum();
    Ok(report)
}

/// The previous set in `dir`, if it can be built on; else why not.
fn previous_set(dir: &Path, key: &ContentKey, block_size: u32) -> Result<Toc, String> {
    let bytes = match std::fs::read(dir.join(TOC_FILE_NAME)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("no previous pak set".into()),
        Err(e) => return Err(format!("the previous table of contents is unreadable ({e})")),
    };
    let toc = Toc::decode(&bytes, &key.0).map_err(|e| format!("the previous pak set is not reusable ({e})"))?;
    if toc.block_size != block_size {
        return Err("the block size changed".into());
    }
    for (index, len) in &toc.containers {
        let path = dir.join(container_file_name(*index));
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() == *len => {}
            _ => return Err(format!("{} is missing or changed", container_file_name(*index))),
        }
    }
    Ok(toc)
}

/// Every container file in `dir`, by index.
fn container_files(dir: &Path) -> Result<BTreeMap<u32, String>, PakError> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if let Some(index) = container_index(&name) {
            out.insert(index, name);
        }
    }
    Ok(out)
}

fn compress(raw: &[u8], level: i32) -> Result<(Vec<u8>, u32, Codec), PakError> {
    let packed = zstd::bulk::compress(raw, level).map_err(|e| PakError::Codec(e.to_string()))?;
    Ok(if packed.len() < raw.len() {
        (packed, raw.len() as u32, Codec::Zstd)
    } else {
        (raw.to_vec(), raw.len() as u32, Codec::Raw)
    })
}

struct ContainerWriter {
    index: u32,
    out: BufWriter<File>,
    len: u64,
}

impl ContainerWriter {
    fn create(dir: &Path, index: u32) -> Result<Self, PakError> {
        let mut out = BufWriter::new(File::create(dir.join(container_file_name(index)))?);
        out.write_all(&container_header(index))?;
        Ok(Self {
            index,
            out,
            len: CONTAINER_HEADER_LEN,
        })
    }

    fn write(&mut self, data: &[u8]) -> Result<(), PakError> {
        if self.len + data.len() as u64 > cipher::MAX_CONTAINER_LEN {
            return Err(PakError::Corrupt("a container would exceed 1 TiB".into()));
        }
        self.out.write_all(data)?;
        self.len += data.len() as u64;
        Ok(())
    }
}

fn finish_container(
    container: ContainerWriter,
    toc: &mut Toc,
    written: &mut Vec<u32>,
    on_event: &mut dyn FnMut(PackEvent),
) -> Result<(), PakError> {
    let ContainerWriter { index, out, len } = container;
    let file = out.into_inner().map_err(|e| PakError::Io(e.into_error()))?;
    file.sync_all()?;
    toc.containers.insert(index, len);
    written.push(index);
    on_event(PackEvent::ContainerWritten {
        file: container_file_name(index),
        bytes: len,
    });
    Ok(())
}
