mod atmosphere_component;
mod camera_component;
mod camera_lens_component;
mod camera_post_process_component;
mod decal_component;
mod environment_rows;
mod fog_component;
mod foliage_component;
mod lens_flare_props;
mod light_component;
mod lod_component;
mod movability;
mod particle_emitter_component;
mod portal_component;
mod post_process_volume_component;
pub mod reflected_list;
mod reflection_capture_component;
mod skeletal_mesh_component;
mod spline_component;
mod sprite_component;
mod static_mesh_component;
mod static_mesh_draw;
mod sublevel_display_component;
mod virtual_geometry_component;
mod voxel_component;
mod voxel_component_runtime;
mod voxel_features;
pub mod voxel_generator_editor;
pub mod voxel_stack_editor;
pub mod voxel_world;
mod water_volume_component;
mod wind_component;

pub use atmosphere_component::*;
pub use camera_component::*;
pub use camera_lens_component::*;
pub use camera_post_process_component::*;
pub use decal_component::*;
pub use environment_rows::*;
pub use fog_component::*;
pub use foliage_component::*;
pub use lens_flare_props::*;
pub use light_component::*;
pub use lod_component::*;
pub use movability::*;
pub use particle_emitter_component::*;
pub use portal_component::*;
pub use post_process_volume_component::*;
pub use reflection_capture_component::*;
pub use skeletal_mesh_component::*;
pub use spline_component::*;
pub use sprite_component::*;
pub use static_mesh_component::*;
pub use static_mesh_draw::*;
pub use sublevel_display_component::*;
pub use virtual_geometry_component::*;
pub use voxel_component::*;
pub use voxel_features::*;
pub use water_volume_component::*;
pub use wind_component::*;

/// How the components that show an image take in a loaded texture asset
/// (registered with the texture kind, [`crate::texture_assets`]).
pub(crate) fn register_texture_bindings() {
    pulsar_scenedb::register_binding::<SpriteComponent>(
        crate::texture_assets::TEXTURE_ASSET_KIND,
        sprite_component::bind_sprite_texture,
    );
    pulsar_scenedb::register_binding::<DecalComponent>(
        crate::texture_assets::TEXTURE_ASSET_KIND,
        decal_component::bind_decal_texture,
    );
}
