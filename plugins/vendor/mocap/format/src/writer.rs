//! The append-only take writer: a recording streams to disk through it.

use crate::codec::encode_column;
use crate::error::FormatError;
use crate::rig::RigProfile;
use crate::take::{Calibration, Marker, Take, TakeMeta};
use std::io::Write;

/// The file's magic.
pub const MAGIC: [u8; 4] = *b"PMOC";
/// The format version this crate writes and the newest it reads.
pub const VERSION: u16 = 1;
/// Default frames per `BLCK` chunk (two seconds at 60 fps).
pub const DEFAULT_BLOCK_FRAMES: usize = 120;

pub(crate) const FOURCC_META: [u8; 4] = *b"META";
pub(crate) const FOURCC_RIG: [u8; 4] = *b"RIGG";
pub(crate) const FOURCC_CALIBRATION: [u8; 4] = *b"CALB";
pub(crate) const FOURCC_BLOCK: [u8; 4] = *b"BLCK";
pub(crate) const FOURCC_EVENTS: [u8; 4] = *b"EVNT";
pub(crate) const FOURCC_INDEX: [u8; 4] = *b"INDX";

/// Bytes of a chunk header: fourcc, length, checksum.
pub(crate) const CHUNK_HEADER: usize = 12;
/// Bytes of a block's plain header: first frame (u64), frame count (u32).
pub(crate) const BLOCK_HEADER: usize = 12;

struct Counting<W> {
    inner: W,
    written: u64,
}

impl<W: Write> Counting<W> {
    fn put(&mut self, bytes: &[u8]) -> Result<(), FormatError> {
        self.inner.write_all(bytes)?;
        self.written += bytes.len() as u64;
        Ok(())
    }

    fn chunk(&mut self, fourcc: [u8; 4], payload: &[u8]) -> Result<(), FormatError> {
        let length = u32::try_from(payload.len()).map_err(|_| FormatError::Malformed {
            chunk: "chunk",
            reason: "payload over 4 GiB".into(),
        })?;
        self.put(&fourcc)?;
        self.put(&length.to_le_bytes())?;
        self.put(&crc32fast::hash(payload).to_le_bytes())?;
        self.put(payload)
    }
}

/// Writes a take chunk by chunk. Dropping it without [`finish`](Self::finish)
/// leaves a readable file that lacks the index and any partial block.
pub struct TakeWriter<W: Write> {
    out: Counting<W>,
    rig: RigProfile,
    block_frames: usize,
    pending: Vec<Vec<f32>>,
    next_frame: u64,
    index: Vec<(u64, u32, u64)>,
}

impl<W: Write> TakeWriter<W> {
    /// Starts a file: header, `META`, `RIGG`.
    pub fn new(
        out: W,
        meta: &TakeMeta,
        rig: &RigProfile,
        block_frames: usize,
    ) -> Result<Self, FormatError> {
        rig.validate()?;
        let mut counting = Counting { inner: out, written: 0 };
        counting.put(&MAGIC)?;
        counting.put(&VERSION.to_le_bytes())?;
        counting.put(&0u16.to_le_bytes())?;
        counting.chunk(FOURCC_META, &serde_json::to_vec(meta)?)?;
        counting.chunk(FOURCC_RIG, &serde_json::to_vec(rig)?)?;
        Ok(Self {
            out: counting,
            rig: rig.clone(),
            block_frames: block_frames.max(1),
            pending: vec![Vec::new(); rig.columns()],
            next_frame: 0,
            index: Vec::new(),
        })
    }

    /// Frames accepted so far (written or pending).
    pub fn frames(&self) -> u64 {
        self.next_frame + self.pending.first().map_or(0, Vec::len) as u64
    }

    /// Appends one frame: one sample per column, `NaN` for a gap.
    pub fn push_frame(&mut self, frame: &[f32]) -> Result<(), FormatError> {
        if frame.len() != self.pending.len() {
            return Err(FormatError::FrameWidth { got: frame.len(), expected: self.pending.len() });
        }
        for (column, value) in self.pending.iter_mut().zip(frame) {
            column.push(*value);
        }
        if self.pending.first().map_or(0, Vec::len) >= self.block_frames {
            self.flush_block()?;
        }
        Ok(())
    }

    /// Writes a marker now (a crash keeps it).
    pub fn add_marker(&mut self, marker: &Marker) -> Result<(), FormatError> {
        self.out.chunk(FOURCC_EVENTS, &serde_json::to_vec(&[marker])?)
    }

    /// Writes the calibration (the last one written wins on read).
    pub fn set_calibration(&mut self, calibration: &Calibration) -> Result<(), FormatError> {
        self.out.chunk(FOURCC_CALIBRATION, &serde_json::to_vec(calibration)?)
    }

    /// Writes the pending frames as a block and flushes the destination: the
    /// most a crash can lose is what has not passed through here.
    pub fn flush_block(&mut self) -> Result<(), FormatError> {
        let frames = self.pending.first().map_or(0, Vec::len);
        if frames == 0 {
            return Ok(());
        }
        let mut raw = Vec::new();
        for (column, channel_quant) in self.pending.iter().zip(self.column_quantisation()) {
            encode_column(column, channel_quant, &mut raw);
        }
        let mut payload = Vec::with_capacity(raw.len() / 2 + 16);
        payload.extend_from_slice(&self.next_frame.to_le_bytes());
        payload.extend_from_slice(&(frames as u32).to_le_bytes());
        payload.extend_from_slice(&lz4_flex::compress_prepend_size(&raw));
        self.index.push((self.next_frame, frames as u32, self.out.written));
        self.out.chunk(FOURCC_BLOCK, &payload)?;
        self.out.inner.flush()?;
        self.next_frame += frames as u64;
        for column in &mut self.pending {
            column.clear();
        }
        Ok(())
    }

    fn column_quantisation(&self) -> Vec<crate::rig::Quantisation> {
        self.rig
            .channels
            .iter()
            .flat_map(|c| std::iter::repeat(c.quantisation).take(c.kind.width()))
            .collect()
    }

    /// Flushes the last block, writes the index and returns the destination.
    pub fn finish(mut self) -> Result<W, FormatError> {
        self.flush_block()?;
        let mut index = Vec::with_capacity(12 + self.index.len() * 20);
        index.extend_from_slice(&self.next_frame.to_le_bytes());
        index.extend_from_slice(&(self.index.len() as u32).to_le_bytes());
        for (first, frames, offset) in &self.index {
            index.extend_from_slice(&first.to_le_bytes());
            index.extend_from_slice(&frames.to_le_bytes());
            index.extend_from_slice(&offset.to_le_bytes());
        }
        self.out.chunk(FOURCC_INDEX, &index)?;
        self.out.inner.flush()?;
        Ok(self.out.inner)
    }
}

/// Serialises a whole take.
pub fn write_take(take: &Take, block_frames: usize) -> Result<Vec<u8>, FormatError> {
    let mut writer = TakeWriter::new(Vec::new(), &take.meta, &take.rig, block_frames)?;
    if let Some(calibration) = &take.calibration {
        writer.set_calibration(calibration)?;
    }
    for marker in &take.markers {
        writer.add_marker(marker)?;
    }
    let mut frame = vec![0.0; take.columns.len()];
    for i in 0..take.frame_count() {
        for (slot, column) in frame.iter_mut().zip(&take.columns) {
            *slot = column[i];
        }
        writer.push_frame(&frame)?;
    }
    writer.finish()
}
