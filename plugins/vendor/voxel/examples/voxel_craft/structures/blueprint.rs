//! A structure as blocks around an anchor: the voxel planet's [`Stamp`] in
//! this game's blocks, a block a metre cube of voxels, with the footprint
//! and height a site needs.
//!
//! Offsets are `(x, y, z)` in blocks with +Y up and `y = 0` the floor level
//! (the first block over the ground); the structure's footprint is centred
//! on `x = z = 0`.

use crate::blocks::Block;
use crate::config::VOXEL_M;
use helio_pass_voxel_planet::stamp::{cell_of, Stamp};

/// Cells a structure block's side: a metre.
pub fn cells_a_block() -> i32 {
    (1.0 / VOXEL_M).round() as i32
}

/// The cells of the blocks between `a` and `b` (inclusive): lowest and
/// highest cell.
pub fn cells(a: [i32; 3], b: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    let n = cells_a_block();
    let lo = [0, 1, 2].map(|k| a[k].min(b[k]) * n);
    let hi = [0, 1, 2].map(|k| a[k].max(b[k]) * n + n - 1);
    (lo, hi)
}

#[derive(Clone, Debug, Default)]
pub struct Blueprint {
    pub stamp: Stamp,
    /// Half extents of the footprint (blocks from the centre column).
    pub half: [i32; 2],
    /// Blocks the structure reaches above the floor.
    pub height: i32,
}

impl Blueprint {
    pub fn new(half_x: i32, half_z: i32) -> Self {
        Self {
            stamp: Stamp::new(),
            half: [half_x, half_z],
            height: 0,
        }
    }

    pub fn set(&mut self, at: [i32; 3], block: Block) {
        self.fill(at, at, block);
    }

    /// Air where there may be terrain or leaves.
    pub fn clear(&mut self, at: [i32; 3]) {
        self.hollow(at, at);
    }

    /// Fills the box between `a` and `b` (inclusive).
    pub fn fill(&mut self, a: [i32; 3], b: [i32; 3], block: Block) {
        self.height = self.height.max(a[1].max(b[1]) + 1);
        let (lo, hi) = cells(a, b);
        self.stamp.fill(lo, hi, block.material());
    }

    /// Clears the box between `a` and `b` (inclusive).
    pub fn hollow(&mut self, a: [i32; 3], b: [i32; 3]) {
        let (lo, hi) = cells(a, b);
        self.stamp.hollow(lo, hi);
    }

    /// The blocks it holds solid (each read at its centre cell).
    pub fn solid_blocks(&self) -> Vec<[i32; 3]> {
        let (Ok(parts), Some((lo, hi))) = (self.stamp.compile(), self.stamp.bounds()) else {
            return Vec::new();
        };
        let n = cells_a_block();
        let (lo, hi) = (lo.map(|v| v.div_euclid(n)), hi.map(|v| v.div_euclid(n)));
        let mut out = Vec::new();
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    if let Some(Some(_)) = cell_of(&parts, [x, y, z].map(|v| v * n + n / 2)) {
                        out.push([x, y, z]);
                    }
                }
            }
        }
        out
    }

    /// A room's four walls (`a` to `b`, outer box) of `block`, between layers.
    pub fn walls(&mut self, a: [i32; 3], b: [i32; 3], block: Block) {
        self.height = self.height.max(a[1].max(b[1]) + 1);
        let (lo, hi) = cells(a, b);
        let n = cells_a_block() - 1;
        self.stamp
            .fill(lo, [hi[0], hi[1], lo[2] + n], block.material());
        self.stamp
            .fill([lo[0], lo[1], hi[2] - n], hi, block.material());
        self.stamp
            .fill(lo, [lo[0] + n, hi[1], hi[2]], block.material());
        self.stamp
            .fill([hi[0] - n, lo[1], lo[2]], hi, block.material());
    }
}
