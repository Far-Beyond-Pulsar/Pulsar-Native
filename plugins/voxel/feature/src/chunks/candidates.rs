//! Feature candidate blocks (docs/features.md): a spawn's raw candidate on
//! a square is a function of the square alone (its hashes and the
//! terrain's ground under it), so the GPU evaluates candidates by blocks of
//! `BLOCK_SIDE` x `BLOCK_SIDE` squares, once while a pool keeps them
//! (`feature_block_*_main`), and each computed footprint's placements copy
//! its squares from the blocks before exclusion (`tile_features_main`).
//! Footprints share most of their squares (a level-0 footprint's tree
//! squares reach some 150 columns past its 64 each way, and coarser levels
//! keep the same squares), so a candidate is evaluated about once, not once
//! for each footprint and level that keeps it.
//!
//! `feature_blocks` holds the pool, then a frame's block jobs (`*_AT`):
//! each job's node quads (count, origins) and reads, then each computed
//! footprint's table (a header a near spawn: first block, blocks a row,
//! where its block slots start) and the slots.

use super::{Footprint, SlotPool, TILE_BATCH};
use crate::features::{CompiledFeatures, BLOCK_SIDE, MAX_SPAWNS, NEAR_LEVELS};

/// Blocks the pool keeps (512 bytes each), block jobs a frame evaluates at
/// most, and the ground reads a job's candidates make at most (an anchor
/// and four footprint corners a square), so the node quads it keeps.
pub(crate) const BLOCK_POOL: usize = 16384;
pub(crate) const BLOCK_JOBS: usize = 64;
pub(crate) const BLOCK_READS: usize = 5 * 64;
/// Words a block takes in the pool (a word pair a square).
const BLOCK_WORDS: usize = 2 * 64;

pub(crate) const JOBS_AT: usize = BLOCK_POOL * BLOCK_WORDS;
const COUNTS_AT: usize = JOBS_AT + 4 * BLOCK_JOBS;
const QUADS_AT: usize = COUNTS_AT + BLOCK_JOBS;
const READS_AT: usize = QUADS_AT + 2 * BLOCK_JOBS * BLOCK_READS;
pub(crate) const TILES_AT: usize = READS_AT + BLOCK_JOBS * BLOCK_READS;
pub(crate) const SLOTS_AT: usize = TILES_AT + 4 * MAX_SPAWNS * TILE_BATCH;

/// `feature_blocks`' words for `features` (a word without features).
pub(crate) fn words(features: &CompiledFeatures) -> usize {
    if features.is_empty() {
        1
    } else {
        SLOTS_AT + TILE_BATCH * features.blocks
    }
}

/// The WGSL constants of the layout (chunk_generate.wgsl).
pub(crate) fn wgsl_constants() -> String {
    format!(
        "const BLOCK_WORDS: u32 = {BLOCK_WORDS}u;\nconst BLOCK_READS: u32 = {BLOCK_READS}u;\nconst FEATURE_JOBS_AT: u32 = {JOBS_AT}u;\nconst FEATURE_COUNTS_AT: u32 = {COUNTS_AT}u;\nconst FEATURE_QUADS_AT: u32 = {QUADS_AT}u;\nconst FEATURE_READS_AT: u32 = {READS_AT}u;\nconst FEATURE_TILES_AT: u32 = {TILES_AT}u;\nconst FEATURE_SLOTS_AT: u32 = {SLOTS_AT}u;\n"
    )
}

/// A block: spawn `spawn`'s squares from `(bx, by) * BLOCK_SIDE` on `face`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct BlockKey {
    face: u8,
    spawn: u8,
    bx: i32,
    by: i32,
}

/// The blocks the GPU keeps and a frame's jobs.
pub(crate) struct CandidateBlocks {
    pool: SlotPool<BlockKey>,
    /// The frame's jobs: face | spawn << 3, block, pool slot.
    jobs: Vec<[u32; 4]>,
}

/// Each near spawn of footprint `fp`'s blocks: the first, and how many
/// each way.
fn spans(
    features: &CompiledFeatures,
    fp: Footprint,
) -> impl Iterator<Item = (usize, [i32; 2], [i32; 2])> + '_ {
    let near = if fp.level < NEAR_LEVELS {
        features.near(fp.level)
    } else {
        0
    };
    let first = [fp.cx * 64 << fp.level, fp.cy * 64 << fp.level];
    (0..near).map(move |s| {
        let (g, side) = features.squares(fp.level, s, first);
        let lo = g.map(|g| g.div_euclid(BLOCK_SIDE));
        let hi = g.map(|g| (g + side - 1).div_euclid(BLOCK_SIDE));
        (s, lo, [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1])
    })
}

/// Footprint `fp`'s blocks, spawn by spawn, row by row.
fn blocks(features: &CompiledFeatures, fp: Footprint) -> impl Iterator<Item = BlockKey> + '_ {
    spans(features, fp).flat_map(move |(s, lo, n)| {
        (0..n[1]).flat_map(move |y| {
            (0..n[0]).map(move |x| BlockKey {
                face: fp.face,
                spawn: s as u8,
                bx: lo[0] + x,
                by: lo[1] + y,
            })
        })
    })
}

impl CandidateBlocks {
    pub(crate) fn new() -> Self {
        Self {
            pool: SlotPool::new(BLOCK_POOL),
            jobs: Vec::new(),
        }
    }

    /// The blocks footprint `fp`'s squares span kept for `frame`, a job
    /// queued for each the pool lacks: false when the frame's jobs or the
    /// pool have no room for them (then `fp` is not computed this frame).
    pub(crate) fn reserve(
        &mut self,
        features: &CompiledFeatures,
        fp: Footprint,
        frame: u64,
    ) -> bool {
        // Those kept first (marked this frame's: no later one evicts them).
        let mut missing = Vec::new();
        for key in blocks(features, fp) {
            if self.pool.slots.contains_key(&key) {
                self.pool.take(key, frame);
            } else {
                missing.push(key);
            }
        }
        if self.jobs.len() + missing.len() > BLOCK_JOBS {
            return false;
        }
        for key in missing {
            match self.pool.take(key, frame) {
                None => return false,
                Some((_, true)) => {}
                Some((slot, false)) => self.jobs.push([
                    u32::from(key.face) | u32::from(key.spawn) << 3,
                    key.bx as u32,
                    key.by as u32,
                    slot,
                ]),
            }
        }
        true
    }

    /// The frame's jobs (taken), and the tables of its computed footprints
    /// `computed` (their blocks reserved this frame): headers from
    /// `TILES_AT`, slots from `SLOTS_AT`.
    pub(crate) fn take_batch(
        &mut self,
        features: &CompiledFeatures,
        computed: impl Iterator<Item = Footprint>,
    ) -> (Vec<[u32; 4]>, Vec<u32>, Vec<u32>) {
        let mut tiles = Vec::new();
        let mut slots = Vec::new();
        for fp in computed {
            let at = tiles.len();
            tiles.resize(at + 4 * MAX_SPAWNS, 0);
            for (s, lo, n) in spans(features, fp) {
                tiles[at + 4 * s..at + 4 * s + 4].copy_from_slice(&[
                    lo[0] as u32,
                    lo[1] as u32,
                    n[0] as u32,
                    slots.len() as u32,
                ]);
                for y in 0..n[1] {
                    for x in 0..n[0] {
                        let key = BlockKey {
                            face: fp.face,
                            spawn: s as u8,
                            bx: lo[0] + x,
                            by: lo[1] + y,
                        };
                        slots.push(self.pool.slots[&key]);
                    }
                }
            }
        }
        (std::mem::take(&mut self.jobs), tiles, slots)
    }
}
