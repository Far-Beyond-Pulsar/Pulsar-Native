//! Virtual geometry GPU culling and selection (plan: `.agents/plans/virtual-geometry.md`
//! sections 2.1, 4, 5.3).
//!
//! One instance-agnostic traversal answers "which clusters of which instances
//! get rasterised this frame, and by which tier":
//!
//! 1. A persistent-workgroup job queue starts at the global instance BVH
//!    (T4, `"vg_gcbvh_nodes"`/`"vg_gcbvh_header"`; stubbed by
//!    `shaders/vg_bvh_stub.wgsl` until it lands), descends to instances, then
//!    into each instance's per-asset 4-wide cluster-group hierarchy
//!    ([`helio_vg_format::GpuHierarchyNode`]). There is no per-instance
//!    dispatch and no per-instance cluster list: a job is `{instance, node}`.
//! 2. Every job tests its own bound: frustum, screen-space error entry test
//!    (`project(max_parent_error) > tau`), two-phase Hi-Z occlusion.
//! 3. A cluster is drawn iff `project(self) <= tau && project(parent) > tau`
//!    (scaled by `lod_error_scale * instance_scale`), after the cone cull
//!    (`INST_CONE_CULL`) and its own frustum/occlusion test. If the pages of
//!    the clusters it was simplified from are not resident it is drawn
//!    *instead* (`project(parent) > tau` alone) and those pages are requested.
//! 4. Survivors are written, with atomics, into bins (below), together with
//!    indirect draw/dispatch args for the raster tiers.
//!
//! # Two phases and the Hi-Z hook
//!
//! [`VgCullEarlyPass`] and [`VgCullLatePass`] are two graph passes sharing one
//! [`VgCullCore`]. The graph order is:
//!
//! ```text
//! VgCullEarlyPass   phase 0: re-draw what was visible last frame (tested against
//!                   last frame's Hi-Z), defer the rest; writes "vg_cull_early"
//! <tier raster, early>   draws phase-0 args (`phase 0` ranges/args below)
//! <Hi-Z build>      MUST publish a Hi-Z from the phase-0 depth as `KEY_HIZ_LATE`
//! VgCullLatePass    phase 1: retest deferred work against that Hi-Z; writes "vg_cull"
//! <tier raster, late>    draws the phase-1 delta (`phase 1` ranges/args below)
//! ```
//!
//! If `KEY_HIZ_LATE` is absent phase 1 skips occlusion (correct, just not
//! culled). `VgCullConfig::two_phase_occlusion = false` disables occlusion and
//! the history entirely; the late pass then does nothing.
//!
//! # Output buffers
//!
//! All published through [`VgCullFrameData`] (registry keys `"vg_cull_early"`
//! and `"vg_cull"`). Words are `u32`.
//!
//! | buffer | layout |
//! |---|---|
//! | `visible_clusters` | `array<vec2<u32>>` = `{instance_id, cluster_slot}`. Eight bins of `bin_capacity` entries: entry `k` of bin `b` is at `b * bin_capacity + k`, and that **unified index** is the "visible cluster index" of the vis-buffer id (`helio_vg_format::vis_pack`). `instance_id` is a row of `vg_instances`, `cluster_slot` is `helio_vg_format::cluster_slot(pool_slot, local)` = `pool_slot << 11 | local` (page word base `pool_slot * 32768`). |
//! | `state` | `array<u32>`, see [`state`]: bin counts, [`stats`], queue cursors, phase-0 counts, **ranges** and **indirect args**. |
//! | page requests | Not a buffer of this pass: unresident pages are requested and used pages touched directly in `helio-vg-stream`'s `vg_feedback` (`vg_request_page`, `vg_use_or_request_page`), which the store reads back. `stats::PAGE_REQUESTS` is no longer counted. |
//!
//! Bins ([`BIN_HW_32`] ..): `0..=3` hardware raster by triangle-count class
//! (`<=32`, `<=64`, `<=96`, `<=128` -> 96/192/288/384 vertices per cluster),
//! `4` = software micro-raster (projected average triangle edge
//! `<= sw_edge_px`), `5` = dense candidates (SW-class clusters overlapping a
//! tile marked dense in last frame's `dense_tile_mask`, plus
//! `TierLock::Dense`). Hardware-class clusters are never moved to dense:
//! the dense tier re-processes tiles, it does not claim whole clusters.
//! **Alpha-masked clusters** (their section's material has `FLAG_ALPHA_TEST`; needs
//! `VgCullInputs::materials`, otherwise the cull is the opaque-only path with no
//! material loads) leave those bins: `6` = hardware masked (always 384 vertices per
//! cluster, one `draw_indirect` at word 24 of the phase's args block), `7` = software
//! masked (only with [`VgCullConfig::sw_masked`], i.e. the 64-bit atomic raster
//! kernel; dispatch args at word 28). Software- and dense-class masked clusters go to
//! `7`, or to `6` without `sw_masked`; tier locks and `ForceHwRaster` apply to the
//! unmasked class the same way. The dense peel bins every range, so it picks masked
//! clusters up from bins `>= CULL_FIRST_MASKED_BIN`. Two-sided materials skip the
//! normal-cone test (their back faces are visible). `TierLock` overrides apply per instance; `VgMode::ForceHwRaster` routes
//! everything to the hardware bins; `VgMode::Off` produces nothing.
//!
//! **Ranges** (`state[RANGES ..]`, byte offset [`RANGES_BYTE_OFFSET`]): per
//! phase `p`, per bin `b`: `{first_unified_index, count}` at word
//! `RANGES + p * 12 + b * 2`. Phase 0 is `[0, count0)` of each bin, phase 1 is
//! the delta `[count0, count1)`; every consumer draws `first .. first+count`.
//!
//! **Indirect args** (`state[ARGS ..]`, byte offset [`ARGS_BYTE_OFFSET`], one
//! 96-byte block per phase, stride [`ARGS_PHASE_BYTES`]):
//!
//! | block offset | contents |
//! |---|---|
//! | `b * 16`, `b < 4` | `DrawIndirect {vertex_count = 96 * (b + 1), instance_count, first_vertex = 0, first_instance = first_unified_index}`. T6 pulls vertices with `instance_index` = visible cluster index (needs `INDIRECT_FIRST_INSTANCE`). |
//! | `64` | SW `DispatchIndirect {x = min(n, SW_GROUP_WIDTH), y = ceil(n / SW_GROUP_WIDTH), z = 1}`; workgroup `(x, y)` is cluster `first + y * SW_GROUP_WIDTH + x` (guard `< first + n`). |
//! | `80` | Dense candidates, same shape as SW. |
//! | `96` | Masked hardware `DrawIndirect {vertex_count = 384, instance_count, 0, first_instance}` (bin 6). |
//! | `112` | Masked software dispatch, same shape as SW (bin 7). |
//!
//! Because `state` is both a storage buffer here and the indirect source in
//! later passes, a consumer must not bind it as storage in the same dispatch
//! that reads it as the indirect buffer.
//!
//! # Required device limits
//!
//! The main bind group has 20 storage buffers (the material table is the 20th) and one
//! sampled texture; request `max_storage_buffers_per_shader_stage >= 20` (native adapters report far
//! more, WebGPU defaults do not suffice; VG is off where unavailable).
//!
//! # Contracts with neighbouring crates (reconciled in T10a)
//!
//! * `GpuCluster::group_id` of a non-leaf cluster is the asset-local group-ref
//!   index of the group that *produced* it (the hierarchy leaf holding its finer
//!   children); `vg_group_refs[group_ref_base + group_id]` names the child page
//!   to request. Leaf clusters (`FLAG_LEAF`, `group_id == NODE_INVALID`) are
//!   never refined.
//! * A hierarchy leaf (`NODE_LEAF_BIT | i`) is a group ref whose clusters all
//!   share one `parent_error`/`parent_sphere`; a node's sphere contains the
//!   `parent_sphere` of every cluster below it and `max_parent_error[i]` is
//!   the max `parent_error` below (builder invariants; the monotonicity of the
//!   cut depends on them).
//! * Cone: `cutoff = cos(half-angle)` of the normal cone; a cluster is culled
//!   when `dot(c - cam, axis) >= sqrt(1 - cutoff^2) * |c - cam| + r`
//!   ([`helio_vg_format::cone_culled`]). `1.0` disables it.
//! * Streamer invariant (enforced by `helio-vg-stream`'s scheduler from the
//!   `.vgeo` page-dependency table): the resident page set is upward closed, a
//!   page is resident only if the pages holding the parents of its clusters
//!   are. Then every leaf-to-root path draws exactly one cluster.
//! * Instance BVH: `helio-pass-vg-bvh` (`vg_gcbvh_nodes/header/items`, binary
//!   tree, leaf items index `vg_instances` through the items table).

pub mod reference;

mod core;
mod pass;

pub use crate::core::{VgCullBuffers, VgCullConfig, VgCullCore, VgCullFrame, VgCullInputs};
pub use crate::pass::{
    VgCullDynamic, VgCullEarlyPass, VgCullFrameData, VgCullLatePass, VgCullSettingsSource,
};

use helio_vg_format::{GpuCluster, VgMode};

// ── Registry keys ──────────────────────────────────────────────────────────

/// `VgCullInputs` published by the integration (T10) before the early pass.
pub const KEY_INPUTS: &str = "vg_cull_inputs";
/// Hi-Z texture view of the previous frame (helio-pass-hiz output).
pub const KEY_HIZ_EARLY: &str = "hiz";
/// Hi-Z texture view built from the phase-0 depth (a second Hi-Z build,
/// placed between the passes). Optional.
pub const KEY_HIZ_LATE: &str = "vg_hiz_late";
/// [`VgCullFrameData`] after phase 0.
pub const KEY_CULL_EARLY: &str = "vg_cull_early";
/// [`VgCullFrameData`] after phase 1 (final).
pub const KEY_CULL: &str = "vg_cull";
/// Scene-sourced buffer keys (documented names for the integration).
pub const KEY_GCBVH_NODES: &str = "vg_gcbvh_nodes";
pub const KEY_GCBVH_HEADER: &str = "vg_gcbvh_header";
pub const KEY_INSTANCES: &str = "vg_instances";

// ── Bins ───────────────────────────────────────────────────────────────────

pub const BIN_HW_32: u32 = 0;
pub const BIN_HW_64: u32 = 1;
pub const BIN_HW_96: u32 = 2;
pub const BIN_HW_128: u32 = 3;
pub const BIN_SW: u32 = 4;
pub const BIN_DENSE: u32 = 5;
/// Hardware raster of clusters whose material is alpha tested (one bin: every
/// cluster is drawn with `HW_MASKED_VERTICES` vertices, a fragment shader
/// samples the alpha texture and discards).
pub const BIN_HW_MASKED: u32 = 6;
/// Software micro-raster of alpha-tested clusters (64-bit atomic kernel only; see
/// [`FLAG_SW_MASKED`]).
pub const BIN_SW_MASKED: u32 = 7;
/// Vertices drawn per cluster of the masked hardware bin (the 128-triangle class).
pub const HW_MASKED_VERTICES: u32 = 384;
pub const BIN_COUNT: u32 = helio_vg_format::CULL_BIN_COUNT;
/// Vertices drawn per cluster for hardware bin `b`: `96 * (b + 1)`.
pub const fn hw_bin_vertex_count(bin: u32) -> u32 {
    96 * (bin + 1)
}
/// Hardware bin of a triangle count.
pub const fn hw_bin_for_tris(tris: u32) -> u32 {
    if tris <= 32 {
        0
    } else if tris <= 64 {
        1
    } else if tris <= 96 {
        2
    } else {
        3
    }
}

/// Hard ceiling on a bin's capacity: `BIN_COUNT * this < 2^24`, so unified
/// visible indices stay below the 3-RT id base (`1 << 24`, well inside the 25-bit
/// visible cluster index of the vis id).
pub const MAX_BIN_CAPACITY: u32 = 1 << 20;
const _: () = assert!(BIN_COUNT * MAX_BIN_CAPACITY < helio_vg_format::VISIBLE_INDEX_LIMIT);
/// Workgroups per row of the SW/dense indirect dispatches.
pub const SW_GROUP_WIDTH: u32 = helio_vg_format::SW_GROUP_WIDTH;
/// Dense mask tile size in pixels.
pub const DENSE_TILE: u32 = 16;
/// `edge_px = EDGE_FACTOR * projected_sphere_radius_px / sqrt(tris)`: the
/// average triangle edge of a cluster modelled as a disc of its bounding
/// sphere's projected radius (equilateral triangles, area `sqrt(3)/4 e^2`).
pub const EDGE_FACTOR: f32 = 2.7;
/// Error value at or above which a cluster/node is treated as infinitely
/// coarse (root parents).
pub const INF_ERROR: f32 = 1.0e30;
/// Error threshold used by `INST_FORCE_ROOT_ONLY` instances.
pub const ROOT_ONLY_TAU: f32 = 1.0e30;
/// Smallest `lod_error_scale` honoured.
pub const MIN_LOD_ERROR_SCALE: f32 = 1.0e-4;

/// Layout of the `state` buffer, in `u32` words.
pub mod state {
    /// Attempted per-bin counts (clamped by finalize).
    pub const BINS: u32 = 0;
    /// [`stats`](super::stats) counters.
    pub const STATS: u32 = 8;
    /// Queue tails: A, B, deferred.
    pub const Q_TAIL: u32 = 24;
    pub const Q_HEAD: u32 = 28;
    /// Clamped per-bin counts after phase 0.
    pub const PHASE0_COUNTS: u32 = 32;
    pub const RANGES: u32 = helio_vg_format::CULL_STATE_RANGES;
    pub const ARGS: u32 = 72;
    pub const ARGS_PHASE_WORDS: u32 = 32;
    /// Tolerance multiplier chosen by the density solve (f32 bits) and the
    /// triangles it predicted (read back with the stats).
    pub const DENSITY: u32 = 136;
    pub const PREDICTED: u32 = 137;
    pub const WORDS: u32 = 140;
}

pub const RANGES_BYTE_OFFSET: u64 = state::RANGES as u64 * 4;
pub const ARGS_BYTE_OFFSET: u64 = state::ARGS as u64 * 4;
pub const ARGS_PHASE_BYTES: u64 = state::ARGS_PHASE_WORDS as u64 * 4;
pub const RANGES_PHASE_BYTES: u64 = BIN_COUNT as u64 * 8;

/// Byte offset of hardware draw args for `phase` (0/1) and `bin` (0..4).
pub const fn hw_draw_args_offset(phase: u32, bin: u32) -> u64 {
    ARGS_BYTE_OFFSET + phase as u64 * ARGS_PHASE_BYTES + bin as u64 * 16
}
/// Byte offset of the SW dispatch args.
pub const fn sw_dispatch_args_offset(phase: u32) -> u64 {
    ARGS_BYTE_OFFSET + phase as u64 * ARGS_PHASE_BYTES + 64
}
/// Byte offset of the dense dispatch args.
pub const fn dense_dispatch_args_offset(phase: u32) -> u64 {
    ARGS_BYTE_OFFSET + phase as u64 * ARGS_PHASE_BYTES + 80
}
/// Byte offset of the masked hardware draw args (one `DrawIndirect`, vertex
/// count [`HW_MASKED_VERTICES`]) for `phase`.
pub const fn hw_masked_draw_args_offset(phase: u32) -> u64 {
    ARGS_BYTE_OFFSET + phase as u64 * ARGS_PHASE_BYTES + 96
}
/// Byte offset of the masked software dispatch args for `phase`.
pub const fn sw_masked_dispatch_args_offset(phase: u32) -> u64 {
    ARGS_BYTE_OFFSET + phase as u64 * ARGS_PHASE_BYTES + 112
}
/// Byte offset of `{first, count}` for `phase` and `bin`.
pub const fn range_offset(phase: u32, bin: u32) -> u64 {
    RANGES_BYTE_OFFSET + phase as u64 * RANGES_PHASE_BYTES + bin as u64 * 8
}

/// Indices into `state[STATS ..]` (diagnostics; also drive lazy growth).
pub mod stats {
    pub const JOBS: u32 = 0;
    pub const CLUSTERS_TESTED: u32 = 1;
    pub const EMITTED: u32 = 2;
    /// A job was dropped because a queue was full (geometry may be missing).
    pub const JOB_OVERFLOW: u32 = 3;
    /// A cluster was dropped because its bin was full.
    pub const VISIBLE_OVERFLOW: u32 = 4;
    pub const PAGE_REQUESTS: u32 = 5;
    pub const REQUEST_OVERFLOW: u32 = 6;
    pub const DEFERRED: u32 = 7;
    pub const CULLED_FRUSTUM: u32 = 8;
    pub const CULLED_OCCLUSION: u32 = 9;
    pub const CULLED_CONE: u32 = 10;
    pub const CULLED_SSE: u32 = 11;
    pub const SPILLED: u32 = 12;
    /// Peak entries seen in a spill/deferred queue.
    pub const PEAK_QUEUE: u32 = 13;
    /// Triangles in successfully emitted clusters, across both phases.
    pub const TRIANGLES: u32 = 14;
    /// Instances whose roots were drawn directly (whole-asset early-out).
    pub const ROOT_SHORTCUT: u32 = 15;
    pub const COUNT: u32 = 16;
}

/// A decoded stats sample (one to a few frames old: the async readback of
/// `state`), for overlays and the demo's title. `tier_clusters` is the
/// attempted cluster count per bin ([`BIN_HW_32`]..[`BIN_DENSE`]) of both
/// phases.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VgCullStats {
    pub jobs: u32,
    pub clusters_tested: u32,
    pub emitted: u32,
    /// Triangles of the emitted clusters (the selected triangle count).
    pub triangles: u32,
    pub deferred: u32,
    pub culled_frustum: u32,
    pub culled_occlusion: u32,
    pub culled_cone: u32,
    pub culled_sse: u32,
    pub job_overflow: u32,
    pub visible_overflow: u32,
    pub root_shortcuts: u32,
    pub peak_queue: u32,
    /// Screen-wide tolerance multiplier the density solve chose this frame, and the
    /// triangles it predicted at it (before occlusion).
    pub density_scale: f32,
    pub predicted_triangles: u32,
    pub tier_clusters: [u32; BIN_COUNT as usize],
}

impl VgCullStats {
    pub fn from_state_words(words: &[u32]) -> Option<Self> {
        if words.len() < (state::STATS + stats::COUNT) as usize {
            return None;
        }
        let st = |i: u32| words[(state::STATS + i) as usize];
        let mut tier_clusters = [0; BIN_COUNT as usize];
        for (b, slot) in tier_clusters.iter_mut().enumerate() {
            *slot = words[state::BINS as usize + b];
        }
        Some(Self {
            jobs: st(stats::JOBS),
            clusters_tested: st(stats::CLUSTERS_TESTED),
            emitted: st(stats::EMITTED),
            triangles: st(stats::TRIANGLES),
            deferred: st(stats::DEFERRED),
            culled_frustum: st(stats::CULLED_FRUSTUM),
            culled_occlusion: st(stats::CULLED_OCCLUSION),
            culled_cone: st(stats::CULLED_CONE),
            culled_sse: st(stats::CULLED_SSE),
            job_overflow: st(stats::JOB_OVERFLOW),
            visible_overflow: st(stats::VISIBLE_OVERFLOW),
            peak_queue: st(stats::PEAK_QUEUE),
            root_shortcuts: st(stats::ROOT_SHORTCUT),
            density_scale: words
                .get(state::DENSITY as usize)
                .map_or(1.0, |w| f32::from_bits(*w).max(1.0)),
            predicted_triangles: words.get(state::PREDICTED as usize).copied().unwrap_or(0),
            tier_clusters,
        })
    }

    /// Software + dense clusters, and the hardware clusters, of the sample.
    pub fn tier_split(&self) -> (u32, u32, u32) {
        let hw: u32 = self.tier_clusters[..BIN_SW as usize].iter().sum::<u32>()
            + self.tier_clusters[BIN_HW_MASKED as usize];
        (
            hw,
            self.tier_clusters[BIN_SW as usize] + self.tier_clusters[BIN_SW_MASKED as usize],
            self.tier_clusters[BIN_DENSE as usize],
        )
    }

    /// One line for an on-screen overlay or window title.
    pub fn summary(&self) -> String {
        let (hw, sw, dense) = self.tier_split();
        format!(
            "tris {} (solve: x{:.2} tolerance, predicted {}) clusters {}/{} (HW {} SW {} dense {}) culled F{} O{} B{} E{} overflow {}",
            self.triangles,
            self.density_scale,
            self.predicted_triangles,
            self.emitted,
            self.clusters_tested,
            hw,
            sw,
            dense,
            self.culled_frustum,
            self.culled_occlusion,
            self.culled_cone,
            self.culled_sse,
            self.job_overflow + self.visible_overflow,
        )
    }
}

/// `VgMode` as the shader's `params.mode`.
pub const fn mode_bits(mode: VgMode) -> u32 {
    match mode {
        VgMode::Auto => 0,
        VgMode::ForceNonRtFallback => 1,
        VgMode::ForceHwRaster => 2,
        VgMode::Off => 3,
    }
}

/// `params.flags` bits.
pub const FLAG_TWO_PHASE: u32 = 1;
pub const FLAG_DENSE_MASK: u32 = 2;
pub const FLAG_STATS: u32 = 4;
/// The material table is bound: clusters are checked for an alpha-tested
/// material and routed to the masked bins (and two-sided materials skip the
/// normal-cone test). Clear = the opaque-only path, no material loads at all.
pub const FLAG_MASKED: u32 = 8;
/// Masked clusters of the software class may use [`BIN_SW_MASKED`] (the raster
/// has the 64-bit atomic kernel); clear = they go to [`BIN_HW_MASKED`].
pub const FLAG_SW_MASKED: u32 = 16;
/// Set = the per-instance root early-out is OFF (A/B and equivalence tests).
pub const FLAG_NO_ROOT_SHORTCUT: u32 = 32;

// ── Shader assembly ────────────────────────────────────────────────────────

const VG_CULL_WGSL: &str = include_str!("../shaders/vg_cull.wgsl");

/// `VGC_*` constants generated from the Rust values so the two cannot drift.
pub fn wgsl_constants() -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let mut u = |name: &str, v: u32| {
        let _ = writeln!(s, "const {name}: u32 = {v}u;");
    };
    u("VGC_BIN_SW", BIN_SW);
    u("VGC_BIN_DENSE", BIN_DENSE);
    u("VGC_BIN_HW_MASKED", BIN_HW_MASKED);
    u("VGC_BIN_SW_MASKED", BIN_SW_MASKED);
    u("VGC_HW_MASKED_VERTS", HW_MASKED_VERTICES);
    u("VGC_BIN_COUNT", BIN_COUNT);
    u("VGC_ST_BINS", state::BINS);
    u("VGC_ST_STATS", state::STATS);
    u("VGC_ST_Q_TAIL", state::Q_TAIL);
    u("VGC_ST_Q_HEAD", state::Q_HEAD);
    u("VGC_ST_PHASE1", state::PHASE0_COUNTS);
    u("VGC_ST_RANGES", state::RANGES);
    u("VGC_ST_ARGS", state::ARGS);
    u("VGC_ST_DENSITY", state::DENSITY);
    u("VGC_ST_PREDICTED", state::PREDICTED);
    u("VGC_ARGS_PHASE_WORDS", state::ARGS_PHASE_WORDS);
    u("VGC_SW_GROUP_WIDTH", SW_GROUP_WIDTH);
    u("VGC_TILE", DENSE_TILE);
    u("VGC_MODE_FORCE_HW", mode_bits(VgMode::ForceHwRaster));
    u("VGC_MODE_OFF", mode_bits(VgMode::Off));
    u("VGC_FLAG_TWO_PHASE", FLAG_TWO_PHASE);
    u("VGC_FLAG_DENSE_MASK", FLAG_DENSE_MASK);
    u("VGC_FLAG_STATS", FLAG_STATS);
    u("VGC_FLAG_MASKED", FLAG_MASKED);
    u("VGC_FLAG_SW_MASKED", FLAG_SW_MASKED);
    u("VGC_FLAG_NO_ROOT_SHORTCUT", FLAG_NO_ROOT_SHORTCUT);
    u("VGC_MAT_ALPHA_TEST", helio_mats::FLAG_ALPHA_TEST);
    u("VGC_MAT_DOUBLE_SIDED", helio_mats::FLAG_DOUBLE_SIDED);
    u("VGC_MAT_ALPHA_BLEND", helio_mats::FLAG_ALPHA_BLEND);
    u("VGC_MAT_CUSTOM", helio_mats::FLAG_HAS_CUSTOM_SHADER);
    u("VGC_CLUSTER_FLAG_LEAF", GpuCluster::FLAG_LEAF);
    for (name, v) in [
        ("JOBS", stats::JOBS),
        ("CLUSTERS_TESTED", stats::CLUSTERS_TESTED),
        ("EMITTED", stats::EMITTED),
        ("JOB_OVERFLOW", stats::JOB_OVERFLOW),
        ("VISIBLE_OVERFLOW", stats::VISIBLE_OVERFLOW),
        ("PAGE_REQUESTS", stats::PAGE_REQUESTS),
        ("REQUEST_OVERFLOW", stats::REQUEST_OVERFLOW),
        ("DEFERRED", stats::DEFERRED),
        ("CULLED_FRUSTUM", stats::CULLED_FRUSTUM),
        ("CULLED_OCCLUSION", stats::CULLED_OCCLUSION),
        ("CULLED_CONE", stats::CULLED_CONE),
        ("CULLED_SSE", stats::CULLED_SSE),
        ("SPILLED", stats::SPILLED),
        ("PEAK_QUEUE", stats::PEAK_QUEUE),
        ("TRIANGLES", stats::TRIANGLES),
        ("ROOT_SHORTCUT", stats::ROOT_SHORTCUT),
    ] {
        u(&format!("VGC_STAT_{name}"), v);
    }
    let _ = writeln!(s, "const VGC_EDGE_FACTOR: f32 = {:?};", EDGE_FACTOR);
    s
}

/// The full WGSL module source (before `helio_core::shader` prelude
/// expansion, which the leading marker in `vg_cull.wgsl` requests).
pub fn shader_source() -> String {
    // The marker line must lead the module source for readability only; the
    // resolver looks for it anywhere.
    format!(
        "//!use helio_prelude
{}
{}
{}
{}
{}
{}
{}",
        wgsl_constants(),
        helio_vg_format::WGSL,
        helio_vg_stream::bindings_wgsl(0, 4, true),
        helio_vg_stream::WGSL,
        helio_vg_stream::WGSL_FEEDBACK,
        helio_pass_vg_bvh::NODES_WGSL,
        VG_CULL_WGSL,
    )
}

/// Every shader source of this crate, fully expanded, for validation tests.
pub fn validation_sources() -> Vec<(&'static str, String)> {
    vec![(
        "vg_cull.wgsl",
        helio_core::shader::resolve(&shader_source()).into_owned(),
    )]
}
