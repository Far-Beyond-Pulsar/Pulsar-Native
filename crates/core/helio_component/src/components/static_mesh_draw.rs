//! What a static mesh instance uploads for drawing (Pulsar-Native#1035,
//! Phase 2).
//!
//! [`StaticMeshComponent`] mirrors its geometry itself (the content-interned
//! `builtin_mesh_vertex`/`builtin_mesh_index` pools). Everything else a
//! renderer needs to draw the instance is derived here from the authored
//! value, as a second GPU registration on the component: the local bounds,
//! the movable flag, and one [`MeshSectionDraw`] per material section with
//! the section's resolved material. SceneDB runs it on every insert,
//! guarded write, removal, despawn and mirror replay of the component, so the
//! rows always describe the current value; nothing calls it.
//!
//! The rows describe the instance alone. Placement (the owner object's
//! transform and visibility) and the instance's enabled state are separate
//! rows on other entities; the renderer's scene join combines them on the
//! GPU each frame they change.
//!
//! Each section's material is resolved from what the world bound into the
//! component ([`crate::mesh_materials`]): an explicit surface override, the
//! bound material asset's row (a graph's textures take slots in the
//! mirror's texture store), or the mesh's imported surface while its
//! material is not loaded. Nothing here reads a file.

use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb_derive::SceneStore;

use super::{StaticMeshComponent, StaticMeshMaterialSlot};
use crate::mesh_cache::MeshSection;
use crate::mesh_materials::{resolve_material, slot_material_id};

/// Pool of every instance's [`MeshSectionDraw`]s; its handle table is
/// `static_mesh_draw_sections::handles`, keyed by the instance entity.
pub const MESH_SECTIONS_BUFFER: &str = "static_mesh_draw_sections";
/// Per instance: local bounding sphere, `[center.xyz, radius]`.
pub const MESH_BOUNDS_BUFFER: &str = "static_mesh_draw_bounds";
/// Per instance: object-row flags (`helio_mesh_api::INSTANCE_FLAG_MOVABLE`).
pub const MESH_FLAGS_BUFFER: &str = "static_mesh_draw_flags";

/// One drawable section of a mesh instance: an index range of the mesh's
/// own indices and the material it draws with.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshSectionDraw {
    /// First index of the section, relative to the mesh's index range.
    pub first_index: u32,
    pub index_count: u32,
    /// Shading class of the material (`helio_mats::MATERIAL_CLASS_*`).
    pub material_class: u32,
    pub graph_hash_lo: u32,
    pub graph_hash_hi: u32,
    pub _pad: [u32; 3],
    pub material: helio_mats::GpuMaterial,
}

// SAFETY: `#[repr(C)]`, every field `Pod`, no padding beyond the explicit
// `_pad` (5 u32s + 3 u32s = 32 bytes ahead of a 16-byte-aligned material).
unsafe impl pulsar_scenedb::page::Pod for MeshSectionDraw {}

/// The derived per-instance draw data; see the module doc.
#[derive(SceneStore, Clone, Debug)]
pub struct StaticMeshDraw {
    #[gpu(buffer = "static_mesh_draw_bounds")]
    pub bounds_local: [f32; 4],
    #[gpu(buffer = "static_mesh_draw_flags")]
    pub flags: u32,
    #[gpu(buffer = "static_mesh_draw_sections")]
    pub sections: Vec<MeshSectionDraw>,
}

impl StaticMeshDraw {
    /// The draw data for `mesh`, resolving each section's material from the
    /// materials bound into it. `mirror` provides the texture store
    /// shader-graph materials register their textures in; without one those
    /// materials fall back to the imported surface.
    pub fn of(mesh: &StaticMeshComponent, mirror: Option<&GpuMirrorHandle>) -> Self {
        let sections: Vec<MeshSection> = if !mesh.mesh_sections.is_empty() {
            mesh.mesh_sections.clone()
        } else if mesh.indices.is_empty() {
            Vec::new()
        } else {
            vec![MeshSection {
                first_index: 0,
                index_count: mesh.indices.len() as u32,
                material_slot: 0,
            }]
        };
        let sections = sections
            .iter()
            .map(|section| {
                let slot = mesh
                    .material_slots
                    .slots
                    .get(section.material_slot as usize);
                let id = slot.map_or(pulsar_scenedb::AssetId::ZERO, slot_material_id);
                let bound = mesh
                    .materials
                    .get(id)
                    .map(|(version, material)| (id, version, material.as_ref()));
                let material = resolve_material(slot, bound, mirror);
                MeshSectionDraw {
                    first_index: section.first_index,
                    index_count: section.index_count,
                    material_class: material.material_class,
                    graph_hash_lo: material.graph_hash as u32,
                    graph_hash_hi: (material.graph_hash >> 32) as u32,
                    _pad: [0; 3],
                    material: material.row,
                }
            })
            .collect();
        Self {
            bounds_local: mesh.bounds_local,
            flags: if helio::Movability::from(mesh.movability).can_move() {
                helio_mesh_api::INSTANCE_FLAG_MOVABLE
            } else {
                0
            },
            sections,
        }
    }
}

fn static_mesh_draw_dispatch(
    mirror: &GpuMirrorHandle,
    row: u32,
    data: *const (),
    is_new_insert: bool,
) {
    // SAFETY: SceneDB reaches this only through `StaticMeshComponent`'s own
    // `ComponentId`, with a pointer to a live `StaticMeshComponent`.
    let mesh = unsafe { &*(data as *const StaticMeshComponent) };
    let draw = StaticMeshDraw::of(mesh, Some(mirror));
    pulsar_scenedb::gpu::write_derived_row(mirror, row, &draw, is_new_insert);
}

fn static_mesh_draw_clear(mirror: &GpuMirrorHandle, row: u32) {
    pulsar_scenedb::gpu::clear_derived_row::<StaticMeshDraw>(mirror, row);
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuMirrorRegistration {
        component_id: pulsar_scenedb::component_id::<StaticMeshComponent>,
        dispatch: static_mesh_draw_dispatch,
    }
}

pulsar_scenedb::pulsar_reflection::inventory::submit! {
    pulsar_scenedb::gpu::GpuClearRegistration {
        component_id: pulsar_scenedb::component_id::<StaticMeshComponent>,
        clear: static_mesh_draw_clear,
    }
}

/// Resolve one slot's material outside a world (tools such as thumbnails
/// and the asset viewer): its material asset is loaded now, on the caller's
/// thread, with the materials plugin's loader. An empty slot, or one whose
/// assignment cannot be loaded (or no enabled plugin loads), draws with the
/// mesh's imported surface (with a warning for a failed load). In a world,
/// the mesh's draw reads the materials the world bound instead.
pub fn resolve_slot_material(
    slot: Option<&StaticMeshMaterialSlot>,
    mirror: Option<&GpuMirrorHandle>,
) -> helio_mats::asset::ResolvedMaterial {
    let id = slot.map_or(pulsar_scenedb::AssetId::ZERO, slot_material_id);
    let loaded = slot
        .filter(|_| !id.is_zero())
        .and_then(|slot| crate::asset_component::asset_source(slot.effective_material_asset()))
        .and_then(|source| {
            match crate::asset_component::load_asset_now(
                helio_mats::asset::MATERIAL_ASSET_KIND,
                &source,
            ) {
                Ok(payload) => payload
                    .downcast::<helio_mats::asset::MaterialPayload>()
                    .ok(),
                Err(error) => {
                    tracing::warn!(%source, %error, "static mesh material could not be loaded; using the imported material");
                    None
                }
            }
        });
    resolve_material(
        slot,
        loaded.as_ref().map(|material| (id, 0, material.0.as_ref())),
        mirror,
    )
}

/// Whether any of `mesh`'s materials, as the world bound them, is a shader
/// graph that reads the frame clock (a `time` node), so its surface
/// animates. Loads, compiles and logs nothing.
pub fn mesh_reads_time(mesh: &StaticMeshComponent) -> bool {
    mesh.material_slots.slots.iter().any(|slot| {
        mesh.materials
            .get(slot_material_id(slot))
            .is_some_and(|(_, material)| material.reads_time())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_inherits_the_mesh_default_until_it_assigns_its_own() {
        let mut slot = StaticMeshMaterialSlot {
            mesh_default_material: "materials/Default.mat".into(),
            ..Default::default()
        };
        assert_eq!(slot.effective_material_asset(), "materials/Default.mat");
        slot.material_asset = "materials/Mine.mat".into();
        assert_eq!(slot.effective_material_asset(), "materials/Mine.mat");
        slot.material_asset = "  ".into();
        assert_eq!(slot.effective_material_asset(), "materials/Default.mat");
    }
}
