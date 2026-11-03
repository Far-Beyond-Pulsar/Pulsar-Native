//! The foliage type, layer, interactor and wind rows as SceneDB components, for standalone hosts
//! (examples, tests): each carries its contract row's exact fields
//! (`same_layout!`), so SceneDB mirrors it into the buffer the passes read.
//! Engines derive these rows from their own components instead. Behind the
//! `host-rows` feature.

use helio_components::same_layout;
use pulsar_scenedb_derive::SceneStore;

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "foliage_types")]
pub struct FoliageTypeComponent {
    #[gpu]
    pub density: f32,
    #[gpu]
    pub height_range: [f32; 2],
    #[gpu]
    pub width_range: [f32; 2],
    #[gpu]
    pub slope_range: [f32; 2],
    #[gpu]
    pub altitude_range: [f32; 2],
    #[gpu]
    pub lod_distances: [f32; 4],
    #[gpu]
    pub wind_response: [f32; 3],
    #[gpu]
    pub interaction_stiffness: f32,
    #[gpu]
    pub material_id: u32,
    #[gpu]
    pub density_layer: u32,
    #[gpu]
    pub kind_and_flags: u32,
    #[gpu]
    pub mesh_or_impostor_id: u32,
    #[gpu]
    pub base_color: u32,
    #[gpu]
    pub roughness_metallic: u32,
    #[gpu]
    pub ground_mask: u32,
    #[gpu]
    pub blades_per_tuft: u32,
    #[gpu]
    pub lean_range: [f32; 2],
    #[gpu]
    pub curve: f32,
    #[gpu]
    pub fall_chance: f32,
    #[gpu]
    pub tip_color: u32,
    #[gpu]
    pub patch_scale: f32,
    #[gpu]
    pub patch_strength: f32,
}

same_layout!(FoliageTypeComponent, crate::FoliageTypeRow, crate::FOLIAGE_TYPES, "foliage_types"; density, height_range, width_range, slope_range, altitude_range, lod_distances, wind_response, interaction_stiffness, material_id, density_layer, kind_and_flags, mesh_or_impostor_id, base_color, roughness_metallic, ground_mask, blades_per_tuft, lean_range, curve, fall_chance, tip_color, patch_scale, patch_strength);

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "foliage_layers")]
pub struct FoliageLayerComponent {
    #[gpu]
    pub bounds_min: [f32; 4],
    #[gpu]
    pub bounds_max: [f32; 4],
}

same_layout!(FoliageLayerComponent, crate::FoliageLayerRow, crate::FOLIAGE_LAYERS, "foliage_layers"; bounds_min, bounds_max);

/// Something that pushes foliage aside: a character's feet, a vehicle, a thrown rock.
/// Blades within `radius` of it lie over away from it, and stay pressed down along its
/// wake (where it was over the last `wake` seconds, from its velocity) and rise again
/// behind it. World positions in f64 halves (`position_hi + position_lo`), so it
/// holds at planet scale: build rows with [`FoliageInteractorComponent::new`].
#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "foliage_interactors")]
pub struct FoliageInteractorComponent {
    /// xyz the world position's f32 part, w the radius (metres; 0: inactive).
    #[gpu]
    pub position_hi_radius: [f32; 4],
    /// xyz the rest of the position (world − hi), w how hard it pushes (1: flat at its
    /// centre for a type of stiffness 1).
    #[gpu]
    pub position_lo_strength: [f32; 4],
    /// xyz its velocity (m/s), w its wake (seconds of track that stay pressed).
    #[gpu]
    pub velocity_wake: [f32; 4],
}

same_layout!(FoliageInteractorComponent, crate::FoliageInteractorRow, crate::FOLIAGE_INTERACTORS, "foliage_interactors"; position_hi_radius, position_lo_strength, velocity_wake);

#[derive(SceneStore, bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
#[gpu(layout = packed, buffer = "foliage_wind")]
pub struct FoliageWindComponent {
    #[gpu]
    pub direction_speed: [f32; 4],
    #[gpu]
    pub gust: [f32; 4],
    #[gpu]
    pub time_prev_time: [f32; 2],
    #[gpu]
    pub _pad: [f32; 2],
}

same_layout!(FoliageWindComponent, crate::FoliageWindRow, crate::FOLIAGE_WIND, "foliage_wind"; direction_speed, gust, time_prev_time, _pad);

impl From<crate::GpuFoliageType> for FoliageTypeComponent {
    fn from(value: crate::GpuFoliageType) -> Self {
        crate::FoliageTypeRow::from(value).into()
    }
}

impl From<FoliageTypeComponent> for crate::GpuFoliageType {
    fn from(value: FoliageTypeComponent) -> Self {
        crate::FoliageTypeRow::from(value).into()
    }
}

impl From<crate::GpuFoliageLayer> for FoliageLayerComponent {
    fn from(value: crate::GpuFoliageLayer) -> Self {
        crate::FoliageLayerRow::from(value).into()
    }
}

impl From<FoliageLayerComponent> for crate::GpuFoliageLayer {
    fn from(value: FoliageLayerComponent) -> Self {
        crate::FoliageLayerRow::from(value).into()
    }
}

