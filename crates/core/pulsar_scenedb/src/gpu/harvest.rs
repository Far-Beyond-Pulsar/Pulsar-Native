//! `HarvestPipeline` — single-scan per-view partition emitting global-row
//! tokens (M2b-b Wave 2 T6, design Rev 2 §5; spec §8.3-8.5, C4).
//!
//! One cell, one view, one scan: [`HarvestPipeline::harvest_cell`] queries a
//! resident cell's positional token run (via the no-allocation §8.1
//! `query_*_in` seams landed in T2) and routes every VALID token into the
//! [`MeshClass`]-selected staging array, offsetting it by the cell's GPU
//! region base. The `NULL_ROW` sentinel is dropped, never offset (§2) — a
//! `region_base + NULL_ROW` value would silently wrap into what looks like a
//! plausible-but-wrong global row, so the routing loop (and the DEI compact
//! kernel) both filter it out BEFORE the add, not after.
//!
//! DEI (§8.5): when a run's hit ratio falls below 25%, the plain
//! filter-and-offset scan is replaced by [`crate::simd::compress_tokens`] (the
//! scalar reference; AVX2 lands in T7), which additionally appends the
//! original run index of every hit to `staging.remap` — the M3-frozen
//! `remap[dense_i] = run index` layout that lets a downstream consumer map a
//! dense output slot back to its source row.

use crate::registry::NULL_ROW;
use crate::snapshot::LivenessSnapshot;
use crate::spatial::SpatialCell;
use crate::Scratchpad;

use super::HarvestPhase;

/// Which GPU-side mesh pipeline a harvested cell's geometry renders through
/// (design Rev 2 §5.2). Routes a harvested run into the matching
/// [`HarvestStaging`] array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshClass {
    Traditional,
    VirtualGeometry,
    HlodProxy,
}

/// The spatial predicate a harvest pass is scanning against — an AABB or a
/// six-plane frustum, mirroring [`SpatialCell::query_aabb_in`]/
/// [`SpatialCell::query_frustum_in`]'s two query shapes.
pub enum View {
    Aabb(crate::spatial::Aabb),
    Frustum(crate::spatial::Frustum),
}

/// Per-view staging arrays (§5.2). Persistent — cleared via [`Self::clear`],
/// never reallocated, once per frame; capacity survives across frames after
/// warm-up (§8.1).
#[derive(Default)]
pub struct HarvestStaging {
    pub traditional: Vec<u32>,
    pub vg: Vec<u32>,
    pub hlod: Vec<u32>,
    /// M3-frozen: `remap[dense_i] = original_run_index`. Only ever grown by
    /// DEI-compacted runs (§8.5); plain-path runs append nothing here.
    pub remap: Vec<u32>,
    pub stats: HarvestStats,
}

#[derive(Default, Clone, Copy, Debug)]
pub struct HarvestStats {
    pub cells: u32,
    pub tokens_valid: u32,
    pub tokens_total: u32,
    pub dei_compacted_runs: u32,
}

impl HarvestStaging {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear every staging array and zero the stats, WITHOUT freeing —
    /// `Vec::clear` on each array (§8.1: capacity is the observable
    /// no-allocation proxy; a fresh `Vec::new()`/`take` here would defeat the
    /// whole point of a persistent staging buffer).
    pub fn clear(&mut self) {
        self.traditional.clear();
        self.vg.clear();
        self.hlod.clear();
        self.remap.clear();
        self.stats = HarvestStats::default();
    }
}

/// Stateless (β single-thread form) driver for one cell/view harvest scan.
/// Holds no state of its own — every buffer it touches (`Scratchpad`,
/// `HarvestStaging`) is caller-owned so the caller controls persistence and
/// threading.
pub struct HarvestPipeline(());

impl HarvestPipeline {
    #[must_use]
    pub fn new() -> Self {
        Self(())
    }

    /// Query one resident inner cell against one view and route its run into
    /// the staging arrays, adding `region_base` to every VALID token (§2 —
    /// the sentinel is never offset; it is dropped here, in both the plain
    /// and DEI-compacted paths). DEI (§8.5): when `valid/total < 0.25` the run
    /// is dense-compacted via [`crate::simd::compress_tokens`], appending a
    /// remap-table segment to `staging.remap`; otherwise a plain
    /// filter-and-offset scan runs. Returns the number of valid tokens
    /// routed (== the query's hit count).
    ///
    /// `_h`: the [`HarvestPhase`] witness — proof this call happens in the
    /// read-only harvest sub-phase (C4), after the frame's Release fence, so
    /// the liveness words captured below observe a stable, published
    /// simulate-phase snapshot.
    pub fn harvest_cell(
        &self,
        cell: &SpatialCell,
        region_base: u32,
        class: MeshClass,
        view: &View,
        pad: &mut Scratchpad,
        staging: &mut HarvestStaging,
        _h: &HarvestPhase,
    ) -> u32 {
        let len = cell.rows_in_use() as usize;
        let (tokens, words) = pad.get_u32_u64(len, len.div_ceil(64));
        let nw = LivenessSnapshot::capture_words(cell.storage().liveness(), len as u32, words);
        let n = match view {
            View::Aabb(q) => cell.query_aabb_in(q, &words[..nw], tokens),
            View::Frustum(f) => cell.query_frustum_in(f, &words[..nw], tokens),
        };
        let dest = match class {
            MeshClass::Traditional => &mut staging.traditional,
            MeshClass::VirtualGeometry => &mut staging.vg,
            MeshClass::HlodProxy => &mut staging.hlod,
        };
        if len > 0 && (n as f32 / len as f32) < 0.25 {
            crate::simd::compress_tokens(&tokens[..len], region_base, dest, &mut staging.remap);
            staging.stats.dei_compacted_runs += 1;
        } else {
            for t in &tokens[..len] {
                if *t != NULL_ROW {
                    dest.push(region_base + *t);
                }
            }
        }
        staging.stats.cells += 1;
        staging.stats.tokens_valid += n;
        staging.stats.tokens_total += len as u32;
        n
    }
}

impl Default for HarvestPipeline {
    fn default() -> Self {
        Self::new()
    }
}
