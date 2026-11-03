//! Brush records: the binary encoding of one brush that edit snapshots
//! (`snapshot`) store, and stable fingerprints of a planet's recipe.
//!
//! A record is 56 bytes, little-endian: kind, shape, operation, material,
//! centre, radius, box height, then its position in its list and a
//! checksum, so a corrupted or reordered record is an error rather than a
//! silently different planet.

use crate::edits::{Brush, BrushOp, BrushShape};
use crate::planet::PlanetRecipe;

pub const RECORD_BYTES: usize = 56;

const KIND_BRUSH: u8 = 1;

/// Stable fingerprint of a recipe (FNV-1a over its canonical JSON).
pub fn recipe_fingerprint(recipe: &PlanetRecipe) -> u64 {
    fnv64(recipe.to_json().as_bytes())
}

/// FNV-1a of `bytes`: the fingerprints of recipes, sources and saved chunks.
pub fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn fnv32(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(*b)).wrapping_mul(0x0100_0193)
    })
}

/// Encode `brush` as record `seq` (its 0-based position in its list).
pub fn encode(seq: u32, brush: &Brush) -> [u8; RECORD_BYTES] {
    let mut out = [0u8; RECORD_BYTES];
    out[0] = KIND_BRUSH;
    out[1] = match brush.shape {
        BrushShape::Sphere => 0,
        BrushShape::Cube => 1,
    };
    out[2] = match brush.op {
        BrushOp::Remove => 0,
        BrushOp::Add => 1,
        BrushOp::Paint => 2,
    };
    out[4..8].copy_from_slice(&brush.material.to_le_bytes());
    for (axis, v) in brush.center.iter().enumerate() {
        out[8 + axis * 8..16 + axis * 8].copy_from_slice(&v.to_le_bytes());
    }
    out[32..40].copy_from_slice(&brush.radius.to_le_bytes());
    out[40..48].copy_from_slice(&brush.height.to_le_bytes());
    out[48..52].copy_from_slice(&seq.to_le_bytes());
    let sum = fnv32(&out[..52]);
    out[52..56].copy_from_slice(&sum.to_le_bytes());
    out
}

/// Decode record `index`; the reason when it is not a valid brush record at
/// that position.
pub(crate) fn decode(index: usize, r: &[u8]) -> Result<Brush, &'static str> {
    if r.len() != RECORD_BYTES {
        return Err("record size");
    }
    let u32_at = |at: usize| u32::from_le_bytes(r[at..at + 4].try_into().unwrap());
    let f64_at = |at: usize| f64::from_le_bytes(r[at..at + 8].try_into().unwrap());
    // Sequence and checksum close the record.
    if u32_at(52) != fnv32(&r[..52]) {
        return Err("checksum");
    }
    if u32_at(48) as usize != index {
        return Err("sequence");
    }
    if r[0] != KIND_BRUSH {
        return Err("record kind");
    }
    let height = f64_at(40);
    let shape = match r[1] {
        0 => BrushShape::Sphere,
        1 => BrushShape::Cube,
        _ => return Err("brush shape"),
    };
    let op = match r[2] {
        0 => BrushOp::Remove,
        1 => BrushOp::Add,
        2 => BrushOp::Paint,
        _ => return Err("brush operation"),
    };
    let center = [f64_at(8), f64_at(16), f64_at(24)];
    let radius = f64_at(32);
    if !center.iter().all(|v| v.is_finite())
        || !(radius.is_finite() && radius > 0.0)
        || !(height.is_finite() && height >= 0.0)
    {
        return Err("brush geometry");
    }
    Ok(Brush {
        center,
        radius,
        shape,
        op,
        material: u32_at(4),
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brush() -> Brush {
        Brush {
            center: [1.5, -2.0, 6_371_000.25],
            radius: 3.0,
            shape: BrushShape::Cube,
            op: BrushOp::Add,
            material: 7,
            height: 2.5,
        }
    }

    #[test]
    fn records_round_trip() {
        let record = encode(4, &brush());
        assert_eq!(decode(4, &record), Ok(brush()));
    }

    #[test]
    fn corruption_and_reordering_are_rejected() {
        let mut record = encode(4, &brush());
        assert_eq!(decode(5, &record), Err("sequence"));
        record[9] ^= 1;
        assert_eq!(decode(4, &record), Err("checksum"));
    }
}
