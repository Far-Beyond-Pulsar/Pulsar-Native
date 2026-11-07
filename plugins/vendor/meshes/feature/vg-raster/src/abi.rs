//! CPU mirrors of the structs the raster shaders read, and constants of the
//! contract with the cull pass (T5), the dense pass (T8) and the shade pass (T9).

use bytemuck::{Pod, Zeroable};

/// Triangle-count capacity of the four Tier 1 draw bins. A cluster with `n`
/// triangles belongs to the first bin whose cap is `>= n`.
pub const HW_BIN_CAPS: [u32; 4] = [32, 64, 96, 128];

/// Index of the Tier 1 bin a cluster with `tris` triangles belongs to.
pub fn hw_bin_for_tris(tris: u32) -> usize {
    HW_BIN_CAPS
        .iter()
        .position(|&c| tris <= c)
        .unwrap_or(HW_BIN_CAPS.len() - 1)
}

/// Value the vis target is cleared to: "no Tier 1 geometry here".
/// It decodes to visible-cluster index `MAX_VISIBLE_CLUSTERS - 1`, tri 127,
/// which is therefore reserved and never a valid hit.
pub const VIS_EMPTY: u32 = 0xffff_ffff;

/// Stride of one row of the scene join's `static_objects` buffer
/// (`StaticObjectRow`, 63 words). Only `transform` (words 4..20) is read.
pub const OBJECT_STRIDE: u64 = 252;

/// One `draw_indirect` argument block (wgpu `DrawIndirectArgs` layout). 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct DrawIndirectArgs {
    pub vertex_count: u32,
    pub instance_count: u32,
    pub first_vertex: u32,
    pub first_instance: u32,
}

/// Small buffer T5 writes describing the software-bin cluster range.
/// 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct SwMeta {
    /// Index of the first software-bin cluster in the visible list.
    pub first: u32,
    /// Number of software-bin clusters.
    pub count: u32,
    pub _pad: [u32; 2],
}

/// Workgroups per dispatch row of the software kernels: workgroup `(x, y)`
/// handles software cluster `y * SW_DISPATCH_ROW + x`.
pub const SW_DISPATCH_ROW: u32 = helio_vg_format::SW_GROUP_WIDTH;

/// `DispatchIndirect` args for `count` software clusters (T5 computes this on
/// the GPU; this is the CPU equivalent used by tests).
pub fn sw_dispatch_args(count: u32) -> [u32; 3] {
    [
        count.min(SW_DISPATCH_ROW),
        count.div_ceil(SW_DISPATCH_ROW).max(1),
        1,
    ]
}

/// Uniform block shared by all raster kernels. 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct VgRasterParams {
    pub width: u32,
    pub height: u32,
    pub tiles_x: u32,
    pub tiles_y: u32,
    pub flags: u32,
    pub tile_bin_cap: u32,
    /// Camera index in the camera buffer.
    pub view_index: u32,
    /// Word offset of `{first, count}` of the software bin inside the `sw_meta`
    /// buffer: 0 for a standalone [`SwMeta`], or the cull's
    /// `state::RANGES + phase * 12 + BIN_SW * 2` when binding its `state`.
    pub sw_meta_word: u32,
}

impl VgRasterParams {
    pub const FLAG_CULL_BACK: u32 = 1;
}

/// Uniform block of `tile_classify`. 32 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct ClassifyParams {
    pub tile_count: u32,
    pub tri_thresh: u32,
    pub hit_thresh: u32,
    pub hysteresis: f32,
    /// Upper bound on dense tiles per frame.
    pub max_dense: u32,
    /// Frames a tile stays dense after its counters last qualified.
    pub latch: u32,
    pub pad: [u32; 2],
}

/// Slots of the `stats` buffer (`u32` counters, cleared every frame).
pub mod stat {
    /// Triangles too large for the exact integer path (should have been Tier 1).
    pub const FLOAT_TRIS: usize = 0;
    /// Triangles crossing a clip plane, skipped by the software tiers.
    pub const CLIPPED_TRIS: usize = 1;
    /// Tile-bin entries dropped because a tile's list was full (fallback path).
    pub const BIN_OVERFLOW: usize = 2;
    pub const COUNT: usize = 4;
}

/// Pixels per tile edge.
pub const TILE_SIZE: u32 = 16;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sw_dispatch_matches_the_cull_split() {
        assert_eq!(sw_dispatch_args(5), [5, 1, 1]);
        assert_eq!(sw_dispatch_args(32768), [32768, 1, 1]);
        assert_eq!(sw_dispatch_args(70000), [32768, 3, 1]);
        assert_eq!(OBJECT_STRIDE, 252);
        assert_eq!(std::mem::size_of::<VgRasterParams>(), 32);
    }
}
