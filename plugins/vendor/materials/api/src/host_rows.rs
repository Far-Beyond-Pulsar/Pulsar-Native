//! The material row as a SceneDB component, for standalone hosts (examples,
//! tests): it carries its contract row's exact fields (`same_layout!`), so
//! SceneDB mirrors it into the [`crate::MATERIALS`] buffer the drawers read.
//! Engines produce the rows from their own material assets. Behind the
//! `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

/// A material authored as a SceneDB component.
///
/// The row deliberately keeps the existing G-buffer shader ABI for this
/// migration step, but its lifetime and updates are now owned by the World.
/// Render passes resolve the packed `"materials"` buffer by key; no renderer
/// material table publication is required for the row itself.
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "materials")]
pub struct MaterialComponent {
    #[gpu]
    pub base_color: [f32; 4],
    #[gpu]
    pub emissive: [f32; 4],
    #[gpu]
    pub roughness_metallic: [f32; 4],
    #[gpu]
    pub tex_base_color: u32,
    #[gpu]
    pub tex_normal: u32,
    #[gpu]
    pub tex_roughness: u32,
    #[gpu]
    pub tex_emissive: u32,
    #[gpu]
    pub tex_occlusion: u32,
    #[gpu]
    pub workflow: u32,
    #[gpu]
    pub flags: u32,
    #[gpu]
    pub material_class: u32,
    #[gpu]
    pub class_params: [f32; 4],
}
same_layout!(MaterialComponent, crate::MaterialRow, crate::MATERIALS, "materials"; base_color, emissive, roughness_metallic, tex_base_color, tex_normal, tex_roughness, tex_emissive, tex_occlusion, workflow, flags, material_class, class_params);

// ---- conversions the rows have, mirrored on their components ----

impl From<crate::GpuMaterial> for MaterialComponent {
    fn from(value: crate::GpuMaterial) -> Self {
        crate::MaterialRow::from(value).into()
    }
}

impl From<MaterialComponent> for crate::GpuMaterial {
    fn from(value: MaterialComponent) -> Self {
        crate::MaterialRow::from(value).into()
    }
}
