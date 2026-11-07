//! The meshes' SceneDB components: static meshes and the draw rows derived
//! from them, skeletal meshes, authored LOD levels and virtual-geometry
//! overrides.

mod lod_component;
mod skeletal_mesh_component;
mod static_mesh_component;
mod static_mesh_draw;
mod virtual_geometry_component;

pub use lod_component::*;
pub use skeletal_mesh_component::*;
pub use static_mesh_component::*;
pub use static_mesh_draw::*;
// Tools resolve a slot outside a world (thumbnails, the asset viewer).
pub use crate::mesh_materials::resolve_slot_material;
pub use virtual_geometry_component::*;
