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
//! Each section names its material slot; the slot's row of the materials'
//! table is the mesh entity's [`crate::mesh_materials::MeshMaterialRows`],
//! which the scene join reads. Nothing of a material is here, so assigning
//! or reloading one rewrites no mesh row. Nothing here reads a file.

use pulsar_scenedb::gpu::GpuMirrorHandle;
use pulsar_scenedb_derive::SceneStore;

use super::StaticMeshComponent;
use crate::mesh_cache::MeshSection;

/// Pool of every instance's [`MeshSectionDraw`]s; its handle table is
/// `static_mesh_draw_sections::handles`, keyed by the instance entity.
pub const MESH_SECTIONS_BUFFER: &str = "static_mesh_draw_sections";
/// Per instance: local bounding sphere, `[center.xyz, radius]`.
pub const MESH_BOUNDS_BUFFER: &str = "static_mesh_draw_bounds";
/// Per instance: object-row flags (`helio_mesh_api::INSTANCE_FLAG_MOVABLE`).
pub const MESH_FLAGS_BUFFER: &str = "static_mesh_draw_flags";

/// One drawable section of a mesh instance: an index range of the mesh's
/// own indices and the material slot it draws with.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshSectionDraw {
    /// First index of the section, relative to the mesh's index range.
    pub first_index: u32,
    pub index_count: u32,
    /// The mesh's material slot (an index of its
    /// [`crate::mesh_materials::MeshMaterialRows`]).
    pub material_slot: u32,
    pub _pad: u32,
}

// SAFETY: `#[repr(C)]`, four `u32`s, no padding.
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
    /// The draw data for `mesh`: each section with its slot's material row.
    pub fn of(mesh: &StaticMeshComponent) -> Self {
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
            .map(|section| MeshSectionDraw {
                first_index: section.first_index,
                index_count: section.index_count,
                material_slot: section.material_slot,
                _pad: 0,
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
    let draw = StaticMeshDraw::of(mesh);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_inherits_the_mesh_default_until_it_assigns_its_own() {
        let mut slot = crate::components::StaticMeshMaterialSlot {
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
