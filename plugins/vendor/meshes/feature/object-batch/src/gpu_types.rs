//! GPU instance/draw-call/object-batch shapes.
//!
//! Owned by `helio-pass-object-batch`, the pass that actually assembles the
//! sorted instance/draw-call/range/shadow-partition data every
//! geometry-drawing pass reads (see [`ObjectBatchFrameData`]'s own doc).
//! Consumers depend on this crate for the shape rather than it being a
//! central-crate-known "generic" type — the shape is real content this pass
//! produces every frame, not a graph/registry mechanism.

use bytemuck::{Pod, Zeroable};

// ── Instance flags (`GpuInstanceData::flags`) ───────────────────────────────
//
// Distinct from a pass's own material `FLAG_*` constants (e.g.
// `GpuMaterial::flags` in `helio-mats`). Same names, different field — check
// which one you are setting.

/// This instance contributes to the shadow atlas.
pub const INSTANCE_FLAG_CASTS_SHADOW: u32 = 1 << 0;

/// This instance receives shadows.
pub const INSTANCE_FLAG_RECEIVES_SHADOW: u32 = 1 << 1;

/// Skip GPU culling for this instance: it is always considered visible.
///
/// Both the frustum test in `indirect_dispatch.wgsl` and the Hi-Z occlusion test in
/// `occlusion_cull.wgsl` pass unconditionally when this bit is set.
///
/// # When this is the right answer
///
/// Culling in this engine is driven by a single world-space bounding **sphere** per
/// instance ([`GpuInstanceData::bounds`]). That representation degrades badly for very
/// large or very flat geometry: a ground plane's sphere has a radius set by its diagonal,
/// so it is enormous relative to the geometry actually inside it, and it both fails to
/// cull anything useful *and* is easy to get wrong in the direction that deletes visible
/// geometry. For a handful of such objects — ground planes, skyboxes, an interior shell
/// the camera lives inside — testing them at all is worth less than the risk of testing
/// them wrongly.
///
/// # When it is not
///
/// This is an escape hatch, not a fix. An instance carrying this flag is submitted every
/// frame no matter where the camera looks, so setting it broadly gives back exactly the
/// GPU-driven culling this engine exists to do. If you find yourself setting it on many
/// objects, the real answer is almost always to split the geometry into pieces whose
/// bounding spheres are tight.
pub const INSTANCE_FLAG_ALWAYS_VISIBLE: u32 = 1 << 2;

/// This instance is `Movable` (participates in the dynamic shadow atlas and
/// per-frame movement bookkeeping) rather than `Static`/`Stationary`. Unset
/// (0) is the common case and matches a `Zeroable`-default row: an instance
/// nobody has ever tagged movable is treated as static, the cheaper and more
/// common case for this pass's shadow-partitioned indirect buffers -- see
/// this crate's module doc for how this bit feeds the static/movable shadow
/// split, computed from a CPU-side `Movability` enum per object.
pub const INSTANCE_FLAG_MOVABLE: u32 = 1 << 3;

/// Another renderer (virtual geometry) draws this instance in the main view:
/// the main-view frustum cull (`indirect_dispatch.wgsl`) skips it. The row
/// stays in the batch, so shadow passes still draw its classic geometry as the
/// shadow proxy. Set by the object batch from the scene buffer
/// `classic_suppressed` (one `u32` per `static_objects` row).
pub const INSTANCE_FLAG_CLASSIC_SUPPRESSED: u32 = 1 << 4;

/// Bit offset of the coordinate-space id within [`GpuInstanceData::flags`].
///
/// # Coordinate spaces
///
/// Every instance is drawn through `coordinate_spaces[space_id] * transform`,
/// where `coordinate_spaces` is a small GPU array of rigid transforms owned
/// by whichever pass assembles the coordinate-space registry (portals/
/// sublevels). Slot 0 is always the identity, so an untagged instance (the
/// overwhelming common case) pays one constant-buffer read and one extra
/// `mat4x4` multiply per vertex — no new pass, no branch, no separate
/// pipeline.
pub const INSTANCE_COORDINATE_SPACE_SHIFT: u32 = 8;

/// Mask for the 8-bit coordinate-space id within [`GpuInstanceData::flags`].
///
/// 256 values are encodable and [`MAX_COORDINATE_SPACES`] (256) are
/// backed by GPU storage (slot 0 is the world, so 255 frames); running out is not a
/// realistic concern.
pub const INSTANCE_COORDINATE_SPACE_MASK: u32 = 0xFF << INSTANCE_COORDINATE_SPACE_SHIFT;

/// Number of coordinate-space slots backed by GPU storage. Slot 0 is always
/// the identity (world space). Mirrors `MAX_COORDINATE_SPACES` in
/// `coordinate_spaces.wgsl` / every shader that binds the buffer — keep in sync.
pub const MAX_COORDINATE_SPACES: u32 = 256;

/// Packs `space` (a coordinate-space slot, see [`INSTANCE_COORDINATE_SPACE_SHIFT`])
/// into `flags`, replacing whatever coordinate-space id was previously set.
///
/// # Panics
/// Debug-asserts `space < MAX_COORDINATE_SPACES`; release builds mask silently.
pub const fn set_coordinate_space(flags: u32, space: u32) -> u32 {
    debug_assert!(space < MAX_COORDINATE_SPACES);
    (flags & !INSTANCE_COORDINATE_SPACE_MASK)
        | ((space << INSTANCE_COORDINATE_SPACE_SHIFT) & INSTANCE_COORDINATE_SPACE_MASK)
}

/// Reads the coordinate-space id packed into `flags` by [`set_coordinate_space`].
/// `0` (world space / identity) for any instance that never had one set.
pub const fn coordinate_space(flags: u32) -> u32 {
    (flags & INSTANCE_COORDINATE_SPACE_MASK) >> INSTANCE_COORDINATE_SPACE_SHIFT
}

/// Per-instance data for GPU-driven rendering. 224 bytes.
///
/// Uploaded once when instances change (dirty tracking), then read-only on GPU.
/// The vertex shader uses `instance_index` to look up this data from a storage buffer.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuInstanceData {
///     transform:    mat4x4<f32>,  // 64 bytes — model matrix
///     normal_mat_0: vec4<f32>,    // 16 bytes — row 0 of normal matrix
///     normal_mat_1: vec4<f32>,    // 16 bytes — row 1
///     normal_mat_2: vec4<f32>,    // 16 bytes — row 2
///     bounds:       vec4<f32>,    // 16 bytes — bounding sphere
///     prev_model:   mat4x4<f32>,  // 64 bytes — previous frame model matrix
///     mesh_id:      u32,          //  4 bytes
///     material_id:  u32,          //  4 bytes
///     flags:        u32,          //  4 bytes
///     lightmap_index: u32,        //  4 bytes — index into lightmap atlas regions buffer
/// }
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuInstanceData {
    /// Model matrix columns 0–3 (column-major, 64 bytes)
    pub model: [f32; 16],
    /// Normal matrix (inverse-transpose of upper-left 3x3, padded to 3×vec4, 48 bytes)
    pub normal_mat: [f32; 12],
    /// Bounding sphere center in world space (xyz) + radius (w)
    pub bounds: [f32; 4],
    /// Previous frame model matrix (column-major, 64 bytes)
    pub prev_model: [f32; 16],
    /// Mesh index into the global mesh table
    pub mesh_id: u32,
    /// Material index into the global material table
    pub material_id: u32,
    /// Flags (bit 0 = casts_shadow, bit 1 = receives_shadow, bit 2 = always_visible,
    /// bits 8-15 = coordinate-space id, see [`set_coordinate_space`]/[`coordinate_space`])
    pub flags: u32,
    /// Index into the lightmap atlas regions buffer (0xFFFFFFFF = no lightmap)
    pub lightmap_index: u32,
    /// Base vertex in the skeletal four-influence stream (zero for rigid meshes).
    pub skin_vertex_offset: u32,
    /// Base matrix in the shared skeletal palette pool.
    pub bone_palette_offset: u32,
    /// Number of matrices available to this instance.
    pub bone_palette_count: u32,
    pub _skin_pad: u32,
}

/// Per-instance AABB in world space for GPU culling. 32 bytes.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuAabb {
///     min: vec3<f32>,
///     _pad0: f32,
///     max: vec3<f32>,
///     _pad1: f32,
/// }
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuInstanceAabb {
    pub min: [f32; 3],
    pub _pad0: f32,
    pub max: [f32; 3],
    pub _pad1: f32,
}

/// A template draw call that the GPU culling compute uses to emit indirect commands.
///
/// Describes one batched draw — all instances in the batch share the same mesh geometry
/// (index range) and are stored consecutively in the instance buffer starting at
/// `first_instance`. Identical (mesh, material) pairs are automatically merged into
/// a single `GpuDrawCall` by this pass, enabling hardware instancing.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuDrawCall {
///     index_count:    u32,
///     first_index:    u32,
///     vertex_offset:  i32,
///     first_instance: u32,  // first index into GpuInstance array for this batch
///     instance_count: u32,  // number of consecutive instances in the batch
/// }
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuDrawCall {
    pub index_count: u32,
    pub first_index: u32,
    pub vertex_offset: i32,
    /// First index into the `GpuInstance` storage buffer for this instanced batch.
    pub first_instance: u32,
    /// Number of instances in the batch (≥ 1). Maximises GPU hardware instancing.
    pub instance_count: u32,
}

/// GPU-side indirect draw command (matches `wgpu::util::DrawIndexedIndirectArgs`).
///
/// The culling compute shader writes these. The render pass reads them via
/// `multi_draw_indexed_indirect`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct DrawIndexedIndirectArgs {
    pub index_count: u32,
    pub instance_count: u32,
    pub first_index: u32,
    pub base_vertex: i32,
    pub first_instance: u32,
}

impl DrawIndexedIndirectArgs {
    /// Creates a culled (invisible) command — instance_count = 0.
    pub const fn culled(
        index_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    ) -> Self {
        Self {
            index_count,
            instance_count: 0,
            first_index,
            base_vertex,
            first_instance,
        }
    }
}
