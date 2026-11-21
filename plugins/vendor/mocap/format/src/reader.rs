//! Reading a take from a byte slice (a read file or a memory map).
//!
//! `open` walks the chunk headers only: it verifies the small chunks and
//! records where each block starts, so a long take opens without decoding it.
//! A block is checksummed and decoded when it is read.

use crate::codec::decode_column;
use crate::error::FormatError;
use crate::rig::RigProfile;
use crate::take::{Calibration, Marker, Take, TakeMeta};
use crate::writer::*;

/// The largest block a reader will inflate (a corrupt length must not allocate).
const MAX_BLOCK_BYTES: usize = 256 << 20;

/// Where one block lives and which frames it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRef {
    pub first_frame: u64,
    pub frames: u32,
    offset: usize,
}

/// What a full read found wrong.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadReport {
    /// The file ended inside a chunk (a take cut off by a crash).
    pub truncated: bool,
    /// Frame ranges `(first, count)` whose block failed and read as gaps.
    pub bad_ranges: Vec<(u64, u32)>,
}

impl ReadReport {
    /// True when nothing was lost.
    pub fn is_clean(&self) -> bool {
        !self.truncated && self.bad_ranges.is_empty()
    }
}

/// An opened take file: everything but the samples, which are read per block.
pub struct TakeFile<'a> {
    bytes: &'a [u8],
    pub meta: TakeMeta,
    pub rig: RigProfile,
    pub calibration: Option<Calibration>,
    pub markers: Vec<Marker>,
    pub blocks: Vec<BlockRef>,
    pub truncated: bool,
}

struct Chunk<'a> {
    fourcc: [u8; 4],
    crc: u32,
    payload: &'a [u8],
    offset: usize,
}

impl Chunk<'_> {
    fn verify(&self) -> Result<(), FormatError> {
        if crc32fast::hash(self.payload) == self.crc {
            Ok(())
        } else {
            Err(FormatError::Checksum {
                fourcc: String::from_utf8_lossy(&self.fourcc).into_owned(),
                offset: self.offset as u64,
            })
        }
    }
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(b)
}

/// The chunk at `offset`, `Ok(None)` at a clean end, `Err(())` when cut off.
fn chunk_at(bytes: &[u8], offset: usize) -> Result<Option<Chunk<'_>>, ()> {
    if offset == bytes.len() {
        return Ok(None);
    }
    let header = bytes.get(offset..offset + CHUNK_HEADER).ok_or(())?;
    let length = le_u32(&header[4..8]) as usize;
    let payload = bytes
        .get(offset + CHUNK_HEADER..offset + CHUNK_HEADER + length)
        .ok_or(())?;
    Ok(Some(Chunk {
        fourcc: [header[0], header[1], header[2], header[3]],
        crc: le_u32(&header[8..12]),
        payload,
        offset,
    }))
}

fn json<T: serde::de::DeserializeOwned>(chunk: &Chunk<'_>, name: &'static str) -> Result<T, FormatError> {
    chunk.verify()?;
    serde_json::from_slice(chunk.payload)
        .map_err(|e| FormatError::Malformed { chunk: name, reason: e.to_string() })
}

impl<'a> TakeFile<'a> {
    /// Opens a take: validates the header and reads every chunk but the samples.
    pub fn open(bytes: &'a [u8]) -> Result<Self, FormatError> {
        if bytes.len() < 8 {
            return Err(FormatError::Truncated("header"));
        }
        if bytes[..4] != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version > VERSION {
            return Err(FormatError::UnsupportedVersion { found: version, supported: VERSION });
        }
        let mut meta = None;
        let mut rig = None;
        let mut file = Self {
            bytes,
            meta: TakeMeta::new("", 1, 1),
            rig: RigProfile::default(),
            calibration: None,
            markers: Vec::new(),
            blocks: Vec::new(),
            truncated: false,
        };
        let mut offset = 8;
        loop {
            let chunk = match chunk_at(bytes, offset) {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(()) => {
                    file.truncated = true;
                    break;
                }
            };
            offset += CHUNK_HEADER + chunk.payload.len();
            file.take_chunk(&chunk, &mut meta, &mut rig)?;
        }
        file.meta = meta.ok_or(FormatError::MissingChunk("META"))?;
        file.rig = rig.ok_or(FormatError::MissingChunk("RIGG"))?;
        file.rig.validate()?;
        Ok(file)
    }

    fn take_chunk(
        &mut self,
        chunk: &Chunk<'_>,
        meta: &mut Option<TakeMeta>,
        rig: &mut Option<RigProfile>,
    ) -> Result<(), FormatError> {
        match chunk.fourcc {
            FOURCC_META => *meta = Some(json(chunk, "META")?),
            FOURCC_RIG => *rig = Some(json(chunk, "RIGG")?),
            FOURCC_CALIBRATION => self.calibration = Some(json(chunk, "CALB")?),
            FOURCC_EVENTS => self.markers.extend(json::<Vec<Marker>>(chunk, "EVNT")?),
            FOURCC_BLOCK if chunk.payload.len() >= BLOCK_HEADER => self.blocks.push(BlockRef {
                first_frame: le_u64(chunk.payload),
                frames: le_u32(&chunk.payload[8..12]),
                offset: chunk.offset,
            }),
            _ => {}
        }
        Ok(())
    }

    /// Total frames across the blocks.
    pub fn frame_count(&self) -> u64 {
        self.blocks.iter().map(|b| b.first_frame + u64::from(b.frames)).max().unwrap_or(0)
    }

    /// Decodes block `index`: one column per rig column, `frames` samples each.
    pub fn read_block(&self, index: usize) -> Result<Vec<Vec<f32>>, FormatError> {
        let block = self.blocks.get(index).ok_or(FormatError::Truncated("block index"))?;
        let chunk = chunk_at(self.bytes, block.offset)
            .ok()
            .flatten()
            .ok_or(FormatError::Truncated("block"))?;
        chunk.verify()?;
        let compressed = &chunk.payload[BLOCK_HEADER..];
        let declared = compressed.get(..4).map(|b| le_u32(b) as usize).unwrap_or(0);
        if declared > MAX_BLOCK_BYTES {
            return Err(malformed("block inflates past the limit"));
        }
        let raw = lz4_flex::decompress_size_prepended(compressed)
            .map_err(|e| malformed(&e.to_string()))?;
        let mut pos = 0;
        (0..self.rig.columns())
            .map(|_| {
                decode_column(&raw, &mut pos, block.frames as usize)
                    .ok_or_else(|| malformed("a column ended early"))
            })
            .collect()
    }

    /// Reads the whole take; a block that fails reads as gaps and is reported.
    pub fn read_all(&self) -> (Take, ReadReport) {
        let total = self.frame_count() as usize;
        let mut take = Take::new(self.meta.clone(), self.rig.clone());
        take.calibration = self.calibration.clone();
        take.markers = self.markers.clone();
        take.columns = vec![vec![f32::NAN; total]; self.rig.columns()];
        let mut report = ReadReport { truncated: self.truncated, bad_ranges: Vec::new() };
        for (i, block) in self.blocks.iter().enumerate() {
            match self.read_block(i) {
                Ok(columns) => {
                    let start = block.first_frame as usize;
                    for (dst, src) in take.columns.iter_mut().zip(columns) {
                        dst[start..start + src.len()].copy_from_slice(&src);
                    }
                }
                Err(_) => report.bad_ranges.push((block.first_frame, block.frames)),
            }
        }
        (take, report)
    }
}

fn malformed(reason: &str) -> FormatError {
    FormatError::Malformed { chunk: "BLCK", reason: reason.to_owned() }
}

/// Reads a whole take from `bytes`.
pub fn read_take(bytes: &[u8]) -> Result<(Take, ReadReport), FormatError> {
    Ok(TakeFile::open(bytes)?.read_all())
}
