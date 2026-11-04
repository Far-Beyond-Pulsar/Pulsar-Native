//! Virtual-geometry raster (plan `.agents/plans/virtual-geometry.md`, T6 + T7).
//!
//! * **Tier 1** (macro geometry): hardware raster by vertex pulling. Four
//!   `draw_indirect` calls, one per triangle-count bin (32/64/96/128);
//!   `first_instance` = index of the bin's first cluster in the visible list,
//!   `vertex_index / 3` = triangle. The vertex shader decodes quantized
//!   positions, the fragment shader writes `vis_pack(visible_cluster, tri)`
//!   to an `R32Uint` target; depth is `Depth32Float` reversed-Z
//!   (`helio_core::depth`). No material evaluation, no mesh shaders.
//! * **Tier 2** (sub-pixel): one workgroup per software-bin cluster; vertices
//!   to shared memory, one thread per triangle, pixel-center coverage on a
//!   snapped integer grid, depth-tested against the Tier 1 depth, then a
//!   64-bit `atomicMax` of `depth_bits << 32 | id` into `sw_keys`. Needs the
//!   [`optional_features`]. Without them the **tiled fallback** runs the same
//!   triangles through `vg_sw_bin.wgsl` (per-tile lists) and
//!   `vg_tile_raster.wgsl` (one workgroup per 16x16 tile, shared-memory
//!   triangle batches, no atomics) and writes a byte-identical `sw_keys`.
//! * **Overlap detection**: per-tile counters (triangles, depth-passing hits)
//!   feed `tile_classify`, which writes a double-buffered dense-tile mask, a
//!   compact dense-tile list and indirect dispatch args.
//!
//! # Contract for T8 (dense) and T9 (shade)
//!
//! * `VgTargets::vis` (`R32Uint`): Tier 1 id word `vis_pack(visible_cluster,
//!   tri)` = `cluster << 7 | tri`; cleared to [`abi::VIS_EMPTY`]
//!   (`0xffffffff`, so visible-cluster index `2^25-1` is reserved).
//! * `VgTargets::depth` (`Depth32Float`): Tier 1 depth, cleared to 0, reversed-Z.
//! * `VgTargets::sw_keys`: `width*height` 64-bit keys, `depth_bits << 32 | id`,
//!   0 = empty; **read as `array<vec2<u32>>`: `.x` = id, `.y` = depth bits** in
//!   both kernel modes. The visible-cluster index inside the id is the index
//!   into the *visible list* (same space as Tier 1). The key's depth was
//!   already tested against Tier 1 depth, so `vis_resolve` picks per pixel:
//!   SW key present => nearer of `bitcast<f32>(key.y)` and the Tier 1 depth
//!   (SW wins ties it passed). All depths are reversed-Z, larger = nearer.
//! * `VgTargets::tile_counters`, `dense_masks` ([`VgTargets::dense_mask_last_frame`]
//!   is the mask to route this frame's clusters with), `dense_list`,
//!   `dense_args` ([x, y, z, count]; one workgroup per dense TILE, workgroup index `y * 65535 + x`).
//! * `shaders/vg_tile_raster.wgsl` is the reusable tile kernel for Tier 3.
//!
//! # Inputs (reconciled in T10a)
//!
//! See [`VgRasterInputs`] and `shaders/vg_bindings.wgsl`. The visible list is
//! the cull's `visible_clusters` (`helio_vg_format::GpuVisibleCluster
//! {instance, cluster_slot}`, unified index `bin * bin_capacity + k`); the
//! four Tier 1 draws and the software dispatch are the cull's indirect args
//! (`SW`: `{min(n, 32768), ceil(n / 32768), 1}`, cluster `first + y * 32768 +
//! x`, guarded `< first + n`); the software `{first, count}` is read from the
//! cull's `state` ranges (`sw_meta_offset_words`). Clusters are read straight
//! from the `helio-vg-stream` page pool: a `cluster_slot` is `pool_slot << 11 |
//! local`, and the `static_objects` rows are the 252-byte scene-join rows.
//!
//! # Position stream encoding
//!
//! ONE decode for every pass: `helio_vg_format::DECODE_WGSL` (asset-wide 18-bit
//! lattice, `vg_asset_quant` origin/step, anchor + per-axis bit widths, page
//! relative attribute offset). [`stream`] only builds real-format test pages.
//!
//! # Limits and behaviour worth knowing
//!
//! * Software triangles crossing a clip plane are skipped and counted in
//!   `stats[stat::CLIPPED_TRIS]`; software triangles with a bbox over 64 px
//!   use f32 edge functions (exact coverage, not bit-watertight) and are
//!   counted in `stats[stat::FLOAT_TRIS]`. Both mean T5 should have used Tier 1.
//! * Back faces are culled (config); mirrored transforms are handled.
//! * 64-bit atomic `max` without a result is the only 64-bit atomic used, so
//!   `SHADER_INT64_ATOMIC_MIN_MAX` (+ `SHADER_INT64`) is sufficient.
//! * The tile fallback caps each tile list at `tile_bin_cap` (default 1024);
//!   overflow drops entries and counts them in `stats[stat::BIN_OVERFLOW]`.

pub mod abi;
pub mod graph;
mod pass;
pub mod reference;
pub mod shaders;
pub mod stream;

pub use pass::{
    optional_features, supports_int64_atomics, VgDenseInputs, VgMaskedInputs, VgRaster,
    VgRasterConfig, VgRasterInputs, VgTargets, DEPTH_FORMAT, VIS_FORMAT,
};

pub use graph::{
    VgHizLatePass, VgRasterEarlyPass, VgRasterFrameData, VgRasterLatePass, VgRasterPasses,
    VgRasterShared, KEY_RASTER, KEY_RASTER_EARLY,
};
