//! Foliage tile residency, GPU placement, tile/cluster culling and per-LOD compaction.
//!
//! This is phase 2 of the foliage stack (plan §6, §10) and the producer half of the
//! grass path: it decides *which blades exist* and *which of them are visible this
//! frame*, and it hands `FoliageGBufferPass` four `draw_indirect` calls plus the buffers
//! those draws index. It draws nothing itself.
//!
//! # The one idea this pass is built around
//!
//! Regenerating every visible blade every frame is the common GPU-grass shortcut and
//! costs about a millisecond at a million blades. Instead the world is a grid of 8 m
//! tiles ([`crate::FOLIAGE_TILE_SIZE_METERS`]), a ring of them around the
//! camera is kept resident in a GPU arena, and placement only runs for tiles *entering*
//! the ring. Steady-state placement cost is therefore zero, and moving-camera cost is
//! proportional to the ring's **perimeter** rather than its area.
//!
//! That property is the entire design, so it is also the thing that is easiest to
//! destroy by accident. [`TileRing::update`] is written to touch only the strips that
//! actually changed and records [`TileRing::last_visited`] so a test can prove it —
//! a "simplification" that rebuilds the resident set each frame would still render
//! correctly and would quietly turn an O(perimeter) pass into an O(area) one.
//!
//! # Stages
//!
//! | Stage | Shader entry | Dispatch | Produces |
//! |---|---|---|---|
//! | 1. Placement | `cs_place` | one workgroup per queued tile, ≤ `max_tiles_per_frame` | `blade_arena`, `tile_table` |
//! | 2a. Tile cull | `cs_tile_cull` | one lane per ring slot | internal visibility mask |
//! | 2b. Cluster cull | `cs_cluster_cull` | one lane per 4×4 blade cluster | `visible_blades`, counters |
//! | 3. Finalize | `cs_finalize` | four lanes | `foliage_indirect` |
//!
//! All four are recorded on `ctx.graphics_cmds()` — the **graphics stream** — and this
//! pass deliberately does not opt into `chain_transparent`. See the header of
//! `shaders/foliage_cull.wgsl` and the plan's §6.2 for why that is not a missed
//! optimisation but a correctness requirement about which frame's Hi-Z gets read.
//!
//! # Zero overhead when absent
//!
//! When the SceneDB `foliage_types` column is absent — no foliage types registered —
//! [`RenderPass::prepare`] early-returns before touching a buffer and
//! [`RenderPass::execute`] records no commands at all. [`foliage_scene_is_present`] is
//! the single predicate both gate on, exposed so the guarantee can be asserted without a
//! GPU. [`FoliagePlacePass::commands_recorded`] counts the frames on which `execute`
//! actually recorded work.
//!
//! # Interface with `FoliageGBufferPass`
//!
//! Four `Arc<wgpu::Buffer>` fields are public and are meant to be cloned into the
//! consumer's constructor by the graph builder, exactly as `ShadowDirtyPass` hands its
//! buffers to `ShadowCullPass`:
//!
//! - [`FoliagePlacePass::blade_arena`] — `GpuBladeInstance[]`
//! - [`FoliagePlacePass::tile_table`] — `GpuFoliageTile[]`
//! - [`FoliagePlacePass::visible_blades`] — `u32` blade indices in four contiguous
//!   per-LOD regions, addressed with [`lod_region_offset`]
//! - [`FoliagePlacePass::foliage_indirect`] — exactly four
//!   [`wgpu::util::DrawIndirectArgs`] at byte offsets 0/16/32/48 in LOD order
//!
//! plus [`FoliagePlacePass::blades_per_tile`], which the consumer needs to recover a
//! blade's owning tile: the arena is partitioned into equal fixed slabs, so
//! `tile_slot = blade_index / blades_per_tile` is exact and O(1).

use bytemuck::{Pod, Zeroable};

pub use helio_foliage_api::*;

pub mod blade;
pub mod scene_rows;
pub use helio_foliage_api::components;
#[cfg(feature = "host-rows")]
pub use helio_foliage_api::host_rows;
pub use scene_rows::FoliageRowSources;
mod frame_data;
mod pass;
mod reference;
mod residency;
pub mod settings;
mod uniforms;
pub mod wind;

pub use components::FoliageInteractorRow;
pub use components::FoliageLayerRow;
pub use components::FoliageTypeRow;
pub use components::FoliageWindRow;
pub use components::FOLIAGE_INTERACTORS;
pub use components::FOLIAGE_LAYERS;
pub use components::FOLIAGE_TYPES;
pub use components::FOLIAGE_WIND;
pub use frame_data::FoliageTerrainViews;
pub use pass::{FoliagePlacePass, INTERACTOR_BYTES, MAX_FOLIAGE_LAYERS, MAX_FOLIAGE_TYPES};
pub use reference::{
    chart_noise, flat_ground, foliage_hash, patch_weight, place_tile_reference, type_grows,
    ReferenceCandidate, ReferenceGround, ReferencePlacement,
};
pub use residency::{RingUpdate, TileRing};
pub use settings::{FoliageSettings, FOLIAGE};
pub use uniforms::{FoliageCullUniforms, PlaceUniforms};
pub use wind::{GpuWind, Wind};

/// Vertices emitted per instance for each foliage LOD, from the plan's §6.3 ladder:
/// a 5-segment blade, a 3-segment blade, a card and a clump card.
///
/// Topology is `TriangleStrip` with **no vertex or index buffer bound**. Per-instance
/// strip restart is guaranteed by the WebGPU spec's primitive-assembly algorithm —
/// assembly runs per instance and a strip is split on the restart value only for
/// *indexed* draws — so a non-indexed instanced strip cannot span an instance boundary
/// and a blade is a single primitive with no degenerate triangles.
///
/// The consumer's `PrimitiveState` must set `strip_index_format: None`: it has no effect
/// on a non-indexed draw, but on Vulkan a `Some` value enables primitive restart, and
/// `None` is the correct state here.
///
/// These are duplicated in `cs_finalize`, which is what actually writes them into the
/// indirect args. Changing one without the other draws a truncated or over-long strip.
///
/// L0 and L1 draw one blade (5 and 3 segments); L2 a whole tuft's blades as cards (up to
/// [`crate::FOLIAGE_MAX_TUFT_BLADES`] × 4 vertices and 2 joins); L3 a tuft as a fan of
/// three blades.
pub const FOLIAGE_LOD_VERTEX_COUNTS: [u32; 4] = [11, 7, 6 * crate::FOLIAGE_MAX_TUFT_BLADES - 2, 13];

/// Interactors the foliage draw looks at, at most: the first gathered each frame
/// (`FoliagePlacePass::interactors`).
pub const FOLIAGE_MAX_INTERACTORS: u32 = 16;

/// Number of `u32` blade indices reserved for each LOD region of `visible_blades`.
///
/// 4 × 2²⁰ indices = 16 MiB total. That is sized so **any one** region can hold the
/// entire 1 M-blade scene the perf gate targets, which is not paranoia: LOD occupancy is
/// extremely bimodal in practice. A camera looking straight down puts nearly every blade
/// in L0; a camera on the horizon puts nearly every blade in L3. Sizing each region for
/// its "fair share" would overflow on the two most ordinary camera angles there are.
///
/// This is a compile-time constant rather than a function of [`FoliageQuality`] because
/// [`lod_region_offset`] is a free function pinned by the interface contract with
/// `FoliageGBufferPass`: the consumer computes region offsets without holding a reference
/// to this pass, and a stride that varied at runtime would have to be plumbed through a
/// uniform, where a mismatch draws the wrong region's indices with no error anywhere. The
/// cost is that `FoliageQuality::Low`, whose whole arena is 4 MiB, still pays 16 MiB
/// here — worth revisiting if mobile ships, but not by making the stride dynamic.
pub const FOLIAGE_VISIBLE_PER_LOD_CAPACITY: u32 = 1 << 20;

/// Byte offset of LOD `lod`'s region inside `visible_blades`.
///
/// Part of the pinned interface contract with `FoliageGBufferPass`. Out-of-range input is
/// clamped to the last region rather than panicking or wrapping, because this is called
/// from draw-recording code where an out-of-range read is worse than a duplicated one.
#[inline]
pub fn lod_region_offset(lod: u32) -> u64 {
    let lod = lod.min(FOLIAGE_LOD_VERTEX_COUNTS.len() as u32 - 1);
    lod as u64 * FOLIAGE_VISIBLE_PER_LOD_CAPACITY as u64 * std::mem::size_of::<u32>() as u64
}

// ── Counter layout ──────────────────────────────────────────────────────────────
//
// One buffer of `FOLIAGE_COUNTER_COUNT` atomics. The first four are the per-LOD visible
// counts the finalize stage turns into instance counts; the rest are the overflow
// telemetry that keeps a full arena from being misdiagnosed as a culling bug.

/// Index of LOD 0's visible-instance counter. LODs 1-3 follow at +1, +2, +3.
pub const COUNTER_VISIBLE_LOD0: usize = 0;

/// Blades dropped because their LOD region of `visible_blades` was full.
///
/// The same contract as `DEFAULT_MAX_PUBLISHED_MESHLETS` and `draw_counters[2]` in
/// virtual geometry, and it exists for the same reason: silently truncated geometry looks
/// exactly like a culling bug, and it is a *stable* one, because in a static scene the
/// same clusters lose the race every frame. Anything reading this buffer should surface
/// this value.
pub const COUNTER_VISIBLE_OVERFLOW: usize = 4;

/// Blades dropped at placement because a tile's arena slab was full.
///
/// Normally zero even under heavy density: the CPU clamps the candidate grid so a tile
/// cannot generate more candidates than its slab holds (see
/// [`FoliagePlacePass::density_scale`]), which thins uniformly instead of leaving a bald
/// corner where the atomic ran out. A non-zero value here therefore means the CPU-side
/// clamp and the shader have drifted, which is a bug rather than a budget.
pub const COUNTER_PLACEMENT_OVERFLOW: usize = 5;

/// Total blades written by placement this frame. Debug telemetry.
pub const COUNTER_PLACED_BLADES: usize = 6;

/// Reserved. Written as zero.
pub const COUNTER_RESERVED: usize = 7;

/// Number of `u32` slots in the counters buffer.
pub const FOLIAGE_COUNTER_COUNT: usize = 8;

/// Default wind displacement extent used to dilate cull bounds, in metres.
///
/// Foliage bounds must be dilated by however far wind can push a vertex, or blades blow
/// outside their tile and get culled while still on screen — the failure Unreal papers
/// over with a manual bounds scale. This is a conservative global default until
/// `wpo_extent` lands per type in `InstanceCullData` (plan §7.2, phase 4).
pub const DEFAULT_WPO_EXTENT_METERS: f32 = 0.5;

/// Whether this frame has any foliage work at all.
///
/// The single predicate both `prepare` and `execute` gate on, and the mechanism behind
/// the plan's §10 zero-overhead guarantee: an absent SceneDB `foliage_types` column
/// means no foliage types are registered, so there is nothing to place, nothing to cull
/// and no reason to record a command. Exposed as a free function so the guarantee is
/// testable on a machine with no GPU.
///
/// Note that this checks for *presence*, not for a non-empty table. A publisher that
/// "helpfully" registers an empty column instead of leaving the column absent turns the
/// free path into four zero-instance draws; the buffer-size check catches that case too,
/// but an absent column is the cheaper contract.
#[inline]
pub fn foliage_scene_is_present(buffers: &helio_core::SceneBufferProjection) -> bool {
    buffers
        .get(pulsar_scenedb::gpu::BufferKey::of("foliage_types"))
        .is_some_and(|handle| handle.buffer.size() >= std::mem::size_of::<GpuFoliageType>() as u64)
}

/// Whether any `foliage_types` row is live (has a density), from an async readback.
///
/// The foliage passes run only while this may hold. A publisher may keep a
/// fixed-capacity table whose rows are all empty (no foliage authored, or all of it
/// disabled), and that table must cost nothing, the same as an absent column.
pub fn foliage_type_liveness() -> helio_core::SceneBufferLiveness {
    helio_core::SceneBufferLiveness::with_row_predicate(|row| {
        row.get(..4)
            .is_some_and(|density| f32::from_le_bytes(density.try_into().unwrap()) > 0.0)
    })
}

/// Mirror of [`wgpu::util::DrawIndirectArgs`] used only for layout assertions and tests.
///
/// `wgpu::util::DrawIndirectArgs` is not `Pod`, so the size contract that the finalize
/// shader depends on cannot be asserted against it directly. This type exists to pin the
/// four-`u32` layout that `foliage_indirect`'s 0/16/32/48 offsets assume.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GpuDrawIndirectArgs {
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}

const _: () = {
    assert!(
        std::mem::size_of::<GpuDrawIndirectArgs>() == 16,
        "DrawIndirectArgs must be 16 bytes for the 0/16/32/48 indirect offsets to hold"
    );
    assert!(
        std::mem::size_of::<wgpu::util::DrawIndirectArgs>() == 16,
        "wgpu's DrawIndirectArgs changed size; foliage_indirect's layout is now wrong"
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_regions_are_contiguous_and_in_lod_order() {
        let stride = FOLIAGE_VISIBLE_PER_LOD_CAPACITY as u64 * 4;
        assert_eq!(lod_region_offset(0), 0);
        assert_eq!(lod_region_offset(1), stride);
        assert_eq!(lod_region_offset(2), stride * 2);
        assert_eq!(lod_region_offset(3), stride * 3);
    }

    #[test]
    fn lod_region_offset_clamps_instead_of_wrapping() {
        // Called from draw-recording code; a duplicated region is survivable, an
        // out-of-bounds one is not.
        assert_eq!(lod_region_offset(4), lod_region_offset(3));
        assert_eq!(lod_region_offset(u32::MAX), lod_region_offset(3));
    }

    #[test]
    fn lod_vertex_counts_match_the_ladder() {
        assert_eq!(FOLIAGE_LOD_VERTEX_COUNTS, [11, 7, 88, 13]);
        assert_eq!(FOLIAGE_LOD_VERTEX_COUNTS.len(), 4);
        // A `TriangleStrip` needs at least 3 vertices to emit anything at all.
        for count in FOLIAGE_LOD_VERTEX_COUNTS {
            assert!(count >= 3);
        }
    }

    #[test]
    fn counter_slots_do_not_collide() {
        let slots = [
            COUNTER_VISIBLE_LOD0,
            COUNTER_VISIBLE_LOD0 + 1,
            COUNTER_VISIBLE_LOD0 + 2,
            COUNTER_VISIBLE_LOD0 + 3,
            COUNTER_VISIBLE_OVERFLOW,
            COUNTER_PLACEMENT_OVERFLOW,
            COUNTER_PLACED_BLADES,
            COUNTER_RESERVED,
        ];
        let mut seen = slots.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), slots.len(), "two counters share a slot");
        assert!(slots.iter().all(|slot| *slot < FOLIAGE_COUNTER_COUNT));
    }

    #[test]
    fn a_single_lod_region_holds_the_perf_gate_scene() {
        // LOD occupancy is bimodal: looking down puts everything in L0, looking at the
        // horizon puts everything in L3. Either extreme must fit.
        assert!(FOLIAGE_VISIBLE_PER_LOD_CAPACITY >= 1_000_000);
    }

    #[test]
    fn draw_indirect_args_layout_is_pinned() {
        assert_eq!(std::mem::size_of::<GpuDrawIndirectArgs>(), 16);
        assert_eq!(std::mem::align_of::<GpuDrawIndirectArgs>(), 4);
        let args = GpuDrawIndirectArgs {
            vertex_count: 11,
            instance_count: 7,
            first_vertex: 0,
            first_instance: 0,
        };
        let bytes: &[u32] = bytemuck::cast_slice(std::slice::from_ref(&args));
        assert_eq!(bytes, &[11, 7, 0, 0]);
    }

    #[test]
    fn zero_overhead_gate_is_closed_on_an_empty_frame() {
        let resources = helio_core::SceneBufferProjection::default();
        assert!(
            !foliage_scene_is_present(&resources),
            "an unwritten foliage slot must not make the pass do work"
        );
    }
}

/// Width of the LOD cross-fade overlap band, in metres.
///
/// Inside this band a cluster is emitted into *both* adjacent LODs, and the consumer
/// blends them with complementary weights so their coverage sums to one blade's worth.
/// Must match `FoliageGBufferPass`'s `lod_fade_band`: this pass decides which clusters get
/// a second instance, the consumer decides the weights, and a mismatch leaves a gap (too
/// little emitted) or a double-density ring (too much).
///
/// Zero is legal and means hard LOD switching — visible banding, but not a crash.
pub const FOLIAGE_LOD_FADE_BAND_METERS: f32 = 4.0;
