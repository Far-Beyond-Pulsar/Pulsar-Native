//! The rows the meshes' components author (objects, render groups, sublevel
//! placements) and the mesh pools they address. The object batch projects
//! them into transient instance and coordinate-space inputs.

/// Mesh vertices: a pool shared by every mesh. A [`StaticObjectRow`]'s
/// `vertex_offset` addresses it. Engines fill it (Helio's standalone hosts use
/// `helio_mesh_host::MeshComponent`).
pub const MESH_VERTICES: helio_core::PoolContract<crate::PackedVertex> =
    helio_core::PoolContract::new("builtin_mesh_vertex");

/// Mesh indices: a pool shared by every mesh, addressed by a
/// [`StaticObjectRow`]'s `first_index`/`index_count`.
pub const MESH_INDICES: helio_core::PoolContract<u32> =
    helio_core::PoolContract::new("builtin_mesh_index");

/// Stable group membership. `group_mask == 0` means always visible.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct RenderGroupRow {
    pub group_mask: u64,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for RenderGroupRow {}

/// The buffer the renderer reads [`RenderGroupRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const RENDER_GROUPS: helio_core::BufferContract<RenderGroupRow> =
    helio_core::BufferContract::new("render_groups");

/// Stable content-space index for an authored sublevel.
///
/// Sublevels are indexed content spaces rather than SceneDB entities. Index
/// zero is the default level and always denotes identity/world space. The
/// index is intentionally a plain scalar here: resolving an index to content
/// and projecting it into a transient GPU coordinate-space slot belongs to a
/// later scene-runtime phase.
pub type SubLevelIndex = u32;

/// The default level's authored sublevel index.
pub const DEFAULT_SUBLEVEL_INDEX: SubLevelIndex = 0;

/// Placement of an indexed sublevel inside its owning level: the portal
/// resolver's CPU input (`helio_pass_portal_cull::SubLevelResolver`).
///
/// This is the direct-instancing primitive for reusable content (for example,
/// an aircraft or room). It is deliberately separate from portal data:
/// portals display a peer portal's context through an aperture and do not
/// become sublevel actors. No pass reads it on the GPU, so it is not a buffer
/// contract; hosts keep it wherever they keep authored placements (a plain
/// SceneDB component is fine).
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct SubLevelActorRow {
    pub sublevel_index: SubLevelIndex,
    pub flags: u32,
    pub transform: [[f32; 4]; 4],
}

impl SubLevelActorRow {
    /// Bit in [`Self::flags`] that enables this actor instance.
    pub const FLAG_ENABLED: u32 = 1 << 0;

    pub fn new(sublevel_index: SubLevelIndex, transform: glam::Mat4) -> Self {
        Self {
            sublevel_index,
            flags: Self::FLAG_ENABLED,
            transform: transform.to_cols_array_2d(),
        }
    }

    pub fn transform(&self) -> glam::Mat4 {
        glam::Mat4::from_cols_array_2d(&self.transform)
    }

    pub fn is_enabled(&self) -> bool {
        self.flags & Self::FLAG_ENABLED != 0
    }
}

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
/// every live `StaticObjectRow` row by `(material_class, graph_hash,
/// mesh_id, material_id)` — a cross-entity operation, not a per-row one.
/// That GPU sort/group/dispatch pipeline is specified but not yet
/// implemented; see the Helio issue tracking it. Until it lands, a
/// consumer must still assemble those arrays itself from a `World` query
/// over this component (a per-frame CPU cost, same category as the
/// pre-existing shadow-atlas scoring — tracked, not silently hidden).
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct StaticObjectRow {
    pub mesh_slot: u32,
    pub mesh_generation: u32,
    pub material_slot: u32,
    pub material_generation: u32,
    pub transform: [[f32; 4]; 4],
    /// Previous frame's `transform`, for TAA/TSR motion vectors. Equal to
    /// `transform` (zero apparent velocity) the frame this row is first
    /// inserted or whenever a caller uses [`Self::with_transform`] without
    /// tracking real per-frame motion -- a quality simplification (a
    /// stationary-looking first frame), never a correctness one.
    pub prev_transform: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 3],
    pub bounds: [f32; 4],
    /// Resolved once at spawn time from `Renderer::mesh_slice` -- see the
    /// struct doc's "Why the draw parameters are stored" section.
    pub index_count: u32,
    pub first_index: u32,
    pub vertex_offset: i32,
    /// Resolved once at spawn time from `Renderer::material_batch_key`.
    pub material_class: u32,
    pub graph_hash_lo: u32,
    pub graph_hash_hi: u32,
    /// See `crate::GpuInstanceData::flags`.
    pub flags: u32,
    /// Base vertex in the parallel four-influence skin stream (zero for rigid meshes).
    pub skin_vertex_offset: u32,
    /// Base joint matrix in the shared palette pool (zero for rigid meshes).
    pub bone_palette_offset: u32,
    /// Number of valid joint matrices for this instance.
    pub bone_palette_count: u32,
    pub _skin_pad: u32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for StaticObjectRow {}

/// The buffer the renderer reads [`StaticObjectRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const STATIC_OBJECTS: helio_core::BufferContract<StaticObjectRow> =
    helio_core::BufferContract::new("static_objects");

impl StaticObjectRow {
    /// Construct an object row from already-resolved SceneDB asset metadata.
    ///
    /// `mesh_slot` and `material_slot` are plain SceneDB row indices. The
    /// renderer is intentionally absent: resolving those indices and the
    /// associated draw range belongs to the asset/component authoring side.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mesh_slot: u32,
        mesh_generation: u32,
        material_slot: u32,
        material_generation: u32,
        transform: glam::Mat4,
        bounds: [f32; 4],
        index_count: u32,
        first_index: u32,
        vertex_offset: i32,
        material_class: u32,
        graph_hash: u64,
        flags: u32,
    ) -> Self {
        Self {
            mesh_slot,
            mesh_generation,
            material_slot,
            material_generation,
            transform: transform.to_cols_array_2d(),
            prev_transform: transform.to_cols_array_2d(),
            normal_mat: normal_matrix_cols(transform),
            bounds,
            index_count,
            first_index,
            vertex_offset,
            material_class,
            graph_hash_lo: graph_hash as u32,
            graph_hash_hi: (graph_hash >> 32) as u32,
            flags,
            skin_vertex_offset: 0,
            bone_palette_offset: 0,
            bone_palette_count: 0,
            _skin_pad: 0,
        }
    }

    pub fn transform(&self) -> glam::Mat4 {
        glam::Mat4::from_cols_array_2d(&self.transform)
    }

    pub fn graph_hash(&self) -> u64 {
        (self.graph_hash_hi as u64) << 32 | self.graph_hash_lo as u64
    }

    /// Re-place this row at a new transform/bounds, recomputing `normal_mat`
    /// to match (it's derived from `transform`, so the two must never drift
    /// apart). `mesh`/`material` and their resolved draw parameters are
    /// static asset properties and are carried over unchanged -- moving an
    /// object never needs to re-resolve them.
    pub fn with_transform(mut self, transform: glam::Mat4, bounds: [f32; 4]) -> Self {
        self.prev_transform = self.transform;
        self.transform = transform.to_cols_array_2d();
        self.normal_mat = normal_matrix_cols(transform);
        self.bounds = bounds;
        self
    }
}

/// Inverse-transpose of `m`'s upper-left 3x3, as three padded columns
/// (matches `crate::GpuInstanceData::normal_mat`'s layout).
fn normal_matrix_cols(m: glam::Mat4) -> [[f32; 4]; 3] {
    let m3 = glam::Mat3::from_mat4(m);
    let inv_t = m3.inverse().transpose();
    let c0 = inv_t.x_axis;
    let c1 = inv_t.y_axis;
    let c2 = inv_t.z_axis;
    [
        [c0.x, c0.y, c0.z, 0.0],
        [c1.x, c1.y, c1.z, 0.0],
        [c2.x, c2.y, c2.z, 0.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn component_layouts_are_stable() {
        assert_eq!(std::mem::size_of::<RenderGroupRow>(), 8);
        assert_eq!(std::mem::size_of::<SubLevelActorRow>(), 72);
        assert_eq!(std::mem::align_of::<SubLevelActorRow>(), 4);
    }

    #[test]
    fn sublevel_actor_round_trips_index_and_transform() {
        let actor =
            SubLevelActorRow::new(7, glam::Mat4::from_translation(glam::vec3(3.0, 4.0, 5.0)));
        assert_eq!(actor.sublevel_index, 7);
        assert_eq!(actor.flags, SubLevelActorRow::FLAG_ENABLED);
        assert_eq!(actor.transform().w_axis, glam::vec4(3.0, 4.0, 5.0, 1.0));
    }
}
