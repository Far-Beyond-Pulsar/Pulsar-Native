//! The meshes' public contract: what their components author (the packed
//! vertex layout and mesh uploads) and the buffers their ray-tracing
//! projection publishes beside the frame's TLAS
//! (`helio_frame_api::rays::SceneRays`): the instance rows
//! ([`RAY_INSTANCES`]) and the proxy pool ([`RAY_PROXY_VERTICES`],
//! [`RAY_PROXY_INDICES`]). No state; a feature that reads meshes depends on
//! this crate, never on the meshes' passes.

mod asset_types;

pub use asset_types::{MeshUpload, PackedVertex, SectionedMeshUpload};

/// Graph key of the rows that follow the frame's TLAS instance order, a
/// [`RayInstance`] an instance.
pub const RAY_INSTANCES: &str = "ray_instances";
/// Graph key of the proxy pool's vertices: the triangles of the instances
/// traced through a proxy ([`RAY_INSTANCE_PROXY`]), packed vertices of
/// [`RAY_PROXY_VERTEX_BYTES`] (a `Float32x3` position, then a `Float32x2`
/// UV), addressed as the mesh pools are (`first_index + 3p`, plus
/// `base_vertex`).
pub const RAY_PROXY_VERTICES: &str = "ray_proxy_vertices";
/// Graph key of the proxy pool's mesh-local `u32` indices.
pub const RAY_PROXY_INDICES: &str = "ray_proxy_indices";
/// Bytes of a proxy pool vertex: position, then UV.
pub const RAY_PROXY_VERTEX_BYTES: u64 = 20;

/// The surface of a TLAS instance, for shading ray hits without a G-buffer
/// (`ray_instances` rows, indexed by TLAS instance index): where its
/// triangles are in the scene's mesh pools and its material's colours.
/// Triangle `p` of the instance is pool indices `first_index + 3p ..`
/// (mesh-local), plus `base_vertex`, into `builtin_mesh_vertex`
/// (`PackedVertex`). Hits are shaded without sampling textures:
/// `base_color` is the material's factor times its base colour texture's
/// mean (as the projection measured it). An instance with a nonzero
/// `transmission` is a thin sheet (glass) its BLAS marks non-opaque: rays
/// pass it, tinted.
///
/// An instance flagged [`RAY_INSTANCE_MASKED`] is a cut-out (an
/// alpha-tested material, row `material` of the scene's `materials`): its
/// BLAS is non-opaque too, and a ray query decides each candidate triangle
/// by the material's alpha at the hit's UV (`PackedVertex::tex_coords0`),
/// as the raster's alpha test does.
///
/// An instance flagged [`RAY_INSTANCE_PROXY`] is traced through a proxy of
/// its geometry (a coarse cut of a virtual-geometry asset): its triangles are
/// in the frame's proxy pool ([`RAY_PROXY_VERTICES`]) instead, positions and UVs
/// only, and a hit's normal is its triangle's face (the winding says
/// outside).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RayInstance {
    pub first_index: u32,
    pub base_vertex: u32,
    /// `RAY_INSTANCE_*` bits.
    pub flags: u32,
    /// The material's row in the scene's `materials` (and its texture
    /// metadata's row).
    pub material: u32,
    /// Linear RGB base colour: the material's factor times its base colour
    /// texture's mean.
    pub base_color: [f32; 4],
    /// The radiance it gives off (RGB: colour times strength).
    pub emissive: [f32; 4],
    /// The linear RGB share of light it lets through (zero: opaque).
    pub transmission: [f32; 4],
}

/// [`RayInstance::flags`]: the instance's triangles are in the frame's proxy
/// pool ([`RAY_PROXY_VERTICES`]), not the scene's mesh pools.
pub const RAY_INSTANCE_PROXY: u32 = 1;
/// [`RayInstance::flags`]: an alpha-tested cut-out (see [`RayInstance`]).
pub const RAY_INSTANCE_MASKED: u32 = 2;
/// [`RayInstance::flags`]: both faces are its outside (a double-sided
/// material): a ray meeting its back has met a surface, not an inside.
pub const RAY_INSTANCE_TWO_SIDED: u32 = 4;

