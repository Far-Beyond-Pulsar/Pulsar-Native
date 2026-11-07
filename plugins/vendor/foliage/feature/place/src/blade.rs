//! A tuft's blades (`tuft_blade` and `block_steps` in foliage_common.wgsl; this is their
//! CPU mirror).
//!
//! A tuft's blades are a function of its seed, the blade's index and the type
//! ([`tuft_blade`]): they fan out from a tight base, lean out more at the rim, bend more
//! towards the tip, and some lie over.

use crate::{foliage_hash, hash_to_unit, GpuFoliageType, FOLIAGE_STACK_CUBES};

/// Cubes a blocky blade steps sideways by at most (L0's strip; L1 takes fewer).
pub const FOLIAGE_STACK_STEPS: u32 = 4;

/// One blade of a tuft (`Blade` in the shader).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Blade {
    pub rim: f32,
    pub angle: f32,
    pub compass: u32,
    pub lean: f32,
    pub curve: f32,
    pub fallen: bool,
    pub height: f32,
    pub width: f32,
}

/// Blade `j` of `n` of the tuft of `seed` (the record's seed half), `turn` (its aux byte
/// over 256) and `height_lerp` (its height byte over 255), of type `ty` (`tuft_blade`).
pub fn tuft_blade(
    ty: &GpuFoliageType,
    seed: u32,
    turn: f32,
    height_lerp: f32,
    j: u32,
    n: u32,
) -> Blade {
    let h = foliage_hash(
        seed.wrapping_mul(0x9e37_79b1)
            .wrapping_add(j.wrapping_mul(0x85eb_ca6b))
            .wrapping_add(0x27d4_eb2f),
    );
    let h2 = foliage_hash(h ^ 0x1656_67b1);
    let unit = hash_to_unit;
    let rim = if j == 0 { 0.0 } else { 0.35 + 0.65 * unit(h) };
    let angle = std::f32::consts::TAU * (turn + (j as f32 + 0.35 * unit(h2)) / n.max(1) as f32);
    let compass = ((turn * 8.0) as u32 + j * 3) % 8;
    let fallen = unit(foliage_hash(h ^ 0x9e37_79b9)) < ty.fall_chance && j != 0;
    let mut lean = (ty.lean_range[0] + (ty.lean_range[1] - ty.lean_range[0]) * rim)
        * (0.75 + 0.5 * unit(h2 >> 3));
    let curve = ty.curve * (0.6 + 0.8 * unit(h >> 5));
    let tuft_height = ty.height_range[0] + (ty.height_range[1] - ty.height_range[0]) * height_lerp;
    let mut height = tuft_height * (1.0 - 0.35 * rim) * (0.75 + 0.5 * unit(h >> 11));
    if fallen {
        lean = lean.max(1.0) * 2.5;
        height *= 0.6;
    }
    let width = ty.width_range[0] + (ty.width_range[1] - ty.width_range[0]) * unit(h2 >> 7);
    Blade {
        rim,
        angle,
        compass,
        lean,
        curve,
        fallen,
        height,
        width,
    }
}

/// How many cubes a blocky blade rises and steps sideways, at most `steps` (`block_steps`).
pub fn block_steps(b: &Blade, cube: f32, scale: f32, steps: u32) -> [u32; 2] {
    let rise = ((b.height * scale / cube).round() as u32).min(FOLIAGE_STACK_CUBES);
    if rise == 0 {
        return [0, 0];
    }
    let side = ((rise as f32 * (b.lean + b.curve).max(0.0)).round() as u32).min(steps);
    [rise, side]
}
