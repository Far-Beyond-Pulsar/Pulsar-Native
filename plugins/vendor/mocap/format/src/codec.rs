//! Column coding: delta in a sortable integer space, zigzag, varint.
//!
//! A column of `f32` samples is mapped to integers that sort like the floats
//! (so a smooth signal has small deltas), the deltas are zigzagged and written
//! as LEB128 varints. Lossless columns round-trip every bit pattern except a
//! gap, which is always the canonical NaN. Quantised columns store 16 bits
//! over the column's range.

use crate::rig::Quantisation;

const TAG_LOSSLESS: u8 = 0;
const TAG_Q16: u8 = 1;
const TAG_GAP: u8 = 2;

/// Maps a signed delta to an unsigned value with small magnitudes first.
pub fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

/// The inverse of [`zigzag`].
pub fn unzigzag(v: u64) -> i64 {
    ((v >> 1) as i64) ^ -((v & 1) as i64)
}

/// Appends `v` as a LEB128 varint.
pub fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// Reads a varint at `*pos`, advancing it; `None` when the buffer ends or the
/// varint is longer than ten bytes.
pub fn get_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *buf.get(*pos)?;
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

fn ordered(bits: u32) -> u32 {
    if bits >> 31 == 1 {
        !bits
    } else {
        bits | 0x8000_0000
    }
}

fn unordered(o: u32) -> u32 {
    if o >> 31 == 1 {
        o & 0x7fff_ffff
    } else {
        !o
    }
}

/// Appends the encoding of `values` to `out`.
pub fn encode_column(values: &[f32], quantisation: Quantisation, out: &mut Vec<u8>) {
    if !values.is_empty() && values.iter().all(|v| v.is_nan()) {
        out.push(TAG_GAP);
    } else if quantisation == Quantisation::Q16 && values.iter().all(|v| v.is_finite()) {
        encode_q16(values, out);
    } else {
        encode_lossless(values, out);
    }
}

fn encode_lossless(values: &[f32], out: &mut Vec<u8>) {
    out.push(TAG_LOSSLESS);
    let mut previous = 0i64;
    for value in values {
        let current = i64::from(ordered(value.to_bits()));
        put_varint(out, zigzag(current - previous));
        previous = current;
    }
}

fn encode_q16(values: &[f32], out: &mut Vec<u8>) {
    out.push(TAG_Q16);
    let min = values.iter().copied().fold(f32::INFINITY, f32::min);
    let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let (min, max) = if values.is_empty() { (0.0, 0.0) } else { (min, max) };
    out.extend_from_slice(&min.to_le_bytes());
    out.extend_from_slice(&max.to_le_bytes());
    let span = max - min;
    let mut previous = 0i64;
    for value in values {
        let q = if span > 0.0 { ((value - min) / span * 65535.0).round() as i64 } else { 0 };
        put_varint(out, zigzag(q - previous));
        previous = q;
    }
}

/// Reads one column of `frames` samples at `*pos`, advancing it.
pub fn decode_column(buf: &[u8], pos: &mut usize, frames: usize) -> Option<Vec<f32>> {
    let tag = *buf.get(*pos)?;
    *pos += 1;
    match tag {
        TAG_GAP => Some(vec![f32::NAN; frames]),
        TAG_LOSSLESS => decode_lossless(buf, pos, frames),
        TAG_Q16 => decode_q16(buf, pos, frames),
        _ => None,
    }
}

fn decode_lossless(buf: &[u8], pos: &mut usize, frames: usize) -> Option<Vec<f32>> {
    let mut out = Vec::with_capacity(frames);
    let mut previous = 0i64;
    for _ in 0..frames {
        previous += unzigzag(get_varint(buf, pos)?);
        let o = u32::try_from(previous).ok()?;
        out.push(f32::from_bits(unordered(o)));
    }
    Some(out)
}

fn read_f32(buf: &[u8], pos: &mut usize) -> Option<f32> {
    let bytes: [u8; 4] = buf.get(*pos..*pos + 4)?.try_into().ok()?;
    *pos += 4;
    Some(f32::from_le_bytes(bytes))
}

fn decode_q16(buf: &[u8], pos: &mut usize, frames: usize) -> Option<Vec<f32>> {
    let min = read_f32(buf, pos)?;
    let max = read_f32(buf, pos)?;
    let span = max - min;
    let mut out = Vec::with_capacity(frames);
    let mut previous = 0i64;
    for _ in 0..frames {
        previous += unzigzag(get_varint(buf, pos)?);
        let q = u16::try_from(previous).ok()?;
        out.push(if span > 0.0 { min + f32::from(q) / 65535.0 * span } else { min });
    }
    Some(out)
}
