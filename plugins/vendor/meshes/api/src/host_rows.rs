//! The meshes' rows as SceneDB components, for standalone hosts (examples,
//! tests, the web demos; `helio-mesh-host` re-exports them): each carries
//! its contract row's exact fields (`same_layout!`), so SceneDB mirrors it
//! into the buffer the passes read. Engines derive the rows from their own
//! components instead. Behind the `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

/// Mesh payload: fills the [`crate::MESH_VERTICES`] and
/// [`crate::MESH_INDICES`] pools.
///
/// The two variable-length fields share SceneDB's keyed geometry pools. Their
/// generated `*_gpu_handle` accessors expose the allocated ranges needed by
/// `StaticObjectRow`; the renderer never owns or resolves a mesh asset.
#[derive(SceneStore, Clone, Debug)]
pub struct MeshComponent {
    #[gpu(buffer = "builtin_mesh_vertex", mirror = Once)]
    pub vertices: Vec<crate::PackedVertex>,
    #[gpu(buffer = "builtin_mesh_index", mirror = Once)]
    pub indices: Vec<u32>,
}

/// Stable group membership. `group_mask == 0` means always visible.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "render_groups")]
pub struct RenderGroupComponent {
    #[gpu]
    pub group_mask: u64,
}
same_layout!(RenderGroupComponent, crate::RenderGroupRow, crate::RENDER_GROUPS, "render_groups"; group_mask);

/// A single-mesh static-object placement: SceneDB-owned mesh/material row
/// indices, a world transform, a culling bound, and the static draw
/// parameters a GPU-driven instancing/culling pipeline needs to read this
/// row directly with no per-frame CPU involvement.
///
/// `SectionedObjectComponent` above covers the multi-material case, which
/// additionally needs the owning `MultiMeshId`/per-section materials tracked
/// elsewhere.
///
/// # Why the draw parameters are stored, not derived per frame
///
/// `index_count`/`first_index`/`vertex_offset` (the mesh's vertex/index
/// range), and `material_class`/`graph_hash` (the material's
/// pipeline-selection key) are authored alongside the row. They are not
/// resolved through a renderer API; the SceneDB asset/ingestion layer owns
/// that resolution before inserting this component.
///
/// `mesh_slot` and `material_slot` are indices into the keyed SceneDB asset
/// buffers. The draw pipeline consumes those plain indices; no renderer
/// handle type is stored or reconstructed here.
///
/// # What's NOT solved by this component alone
///
/// Building the *grouped* `draw_calls`/`instances`/`aabbs` arrays a
/// GPU-driven culling pass reads (batching same-mesh-same-material
/// instances into one indirect draw call, partitioned into contiguous
/// opaque/transparent/forward ranges by `material_class`) requires sorting
/// every live `StaticObjectComponent` row by `(material_class, graph_hash,
/// mesh_id, material_id)` — a cross-entity operation, not a per-row one.
/// That GPU sort/group/dispatch pipeline is specified but not yet
/// implemented; see the Helio issue tracking it. Until it lands, a
/// consumer must still assemble those arrays itself from a `World` query
/// over this component (a per-frame CPU cost, same category as the
/// pre-existing shadow-atlas scoring — tracked, not silently hidden).
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "static_objects")]
pub struct StaticObjectComponent {
    #[gpu]
    pub mesh_slot: u32,
    #[gpu]
    pub mesh_generation: u32,
    #[gpu]
    pub material_slot: u32,
    #[gpu]
    pub material_generation: u32,
    #[gpu]
    pub transform: [[f32; 4]; 4],
    /// Previous frame's `transform`, for TAA/TSR motion vectors. Equal to
    /// `transform` (zero apparent velocity) the frame this row is first
    /// inserted or whenever a caller uses [`Self::with_transform`] without
    /// tracking real per-frame motion -- a quality simplification (a
    /// stationary-looking first frame), never a correctness one.
    #[gpu]
    pub prev_transform: [[f32; 4]; 4],
    #[gpu]
    pub normal_mat: [[f32; 4]; 3],
    #[gpu]
    pub bounds: [f32; 4],
    /// Resolved once at spawn time from `Renderer::mesh_slice` -- see the
    /// struct doc's "Why the draw parameters are stored" section.
    #[gpu]
    pub index_count: u32,
    #[gpu]
    pub first_index: u32,
    #[gpu]
    pub vertex_offset: i32,
    /// Resolved once at spawn time from `Renderer::material_batch_key`.
    #[gpu]
    pub material_class: u32,
    #[gpu]
    pub graph_hash_lo: u32,
    #[gpu]
    pub graph_hash_hi: u32,
    /// See `crate::GpuInstanceData::flags`.
    #[gpu]
    pub flags: u32,
    /// Base vertex in the parallel four-influence skin stream (zero for rigid meshes).
    #[gpu]
    pub skin_vertex_offset: u32,
    /// Base joint matrix in the shared palette pool (zero for rigid meshes).
    #[gpu]
    pub bone_palette_offset: u32,
    /// Number of valid joint matrices for this instance.
    #[gpu]
    pub bone_palette_count: u32,
    #[gpu]
    pub _skin_pad: u32,
}
same_layout!(StaticObjectComponent, crate::StaticObjectRow, crate::STATIC_OBJECTS, "static_objects"; mesh_slot, mesh_generation, material_slot, material_generation, transform, prev_transform, normal_mat, bounds, index_count, first_index, vertex_offset, material_class, graph_hash_lo, graph_hash_hi, flags, skin_vertex_offset, bone_palette_offset, bone_palette_count, _skin_pad);

// ---- row builders, returning the component (see builders.py note) ----

impl StaticObjectComponent {
    pub fn with_transform(self, transform: glam::Mat4, bounds: [f32; 4]) -> Self {
        crate::StaticObjectRow::from(self)
            .with_transform(transform, bounds)
            .into()
    }
}
