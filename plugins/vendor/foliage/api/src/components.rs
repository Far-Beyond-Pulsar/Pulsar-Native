//! SceneDB-owned foliage authoring records.

use crate::{GpuFoliageLayer, GpuFoliageType};

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct FoliageTypeRow {
    pub density: f32,
    pub height_range: [f32; 2],
    pub width_range: [f32; 2],
    pub slope_range: [f32; 2],
    pub altitude_range: [f32; 2],
    pub lod_distances: [f32; 4],
    pub wind_response: [f32; 3],
    pub interaction_stiffness: f32,
    pub material_id: u32,
    pub density_layer: u32,
    pub kind_and_flags: u32,
    pub mesh_or_impostor_id: u32,
    pub base_color: u32,
    pub roughness_metallic: u32,
    pub ground_mask: u32,
    pub blades_per_tuft: u32,
    pub lean_range: [f32; 2],
    pub curve: f32,
    pub fall_chance: f32,
    pub tip_color: u32,
    pub patch_scale: f32,
    pub patch_strength: f32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for FoliageTypeRow {}

/// The buffer the renderer reads [`FoliageTypeRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const FOLIAGE_TYPES: helio_core::BufferContract<FoliageTypeRow> =
    helio_core::BufferContract::new("foliage_types");

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct FoliageLayerRow {
    pub bounds_min: [f32; 4],
    pub bounds_max: [f32; 4],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for FoliageLayerRow {}

/// The buffer the renderer reads [`FoliageLayerRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const FOLIAGE_LAYERS: helio_core::BufferContract<FoliageLayerRow> =
    helio_core::BufferContract::new("foliage_layers");

/// Something that pushes foliage aside: a character's feet, a vehicle, a thrown rock.
/// Blades within `radius` of it lie over away from it, and stay pressed down along its
/// wake (where it was over the last `wake` seconds, from its velocity) and rise again
/// behind it. World positions in f64 halves (`position_hi + position_lo`), so it
/// holds at planet scale: build rows with [`FoliageInteractorRow::new`].
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct FoliageInteractorRow {
    /// xyz the world position's f32 part, w the radius (metres; 0: inactive).
    pub position_hi_radius: [f32; 4],
    /// xyz the rest of the position (world − hi), w how hard it pushes (1: flat at its
    /// centre for a type of stiffness 1).
    pub position_lo_strength: [f32; 4],
    /// xyz its velocity (m/s), w its wake (seconds of track that stay pressed).
    pub velocity_wake: [f32; 4],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for FoliageInteractorRow {}

/// The buffer the renderer reads [`FoliageInteractorRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const FOLIAGE_INTERACTORS: helio_core::BufferContract<FoliageInteractorRow> =
    helio_core::BufferContract::new("foliage_interactors");

impl FoliageInteractorRow {
    /// An interactor at world `position` (the point that presses the ground: feet, a
    /// wheel's contact) moving at `velocity`.
    pub fn new(
        position: glam::DVec3,
        radius: f32,
        strength: f32,
        velocity: glam::Vec3,
        wake: f32,
    ) -> Self {
        let mut row = Self {
            position_hi_radius: [0.0, 0.0, 0.0, radius],
            position_lo_strength: [0.0, 0.0, 0.0, strength],
            velocity_wake: [0.0; 4],
        };
        row.set_motion(position, velocity);
        row.velocity_wake[3] = wake;
        row
    }

    /// Move it to `position`, moving at `velocity`.
    pub fn set_motion(&mut self, position: glam::DVec3, velocity: glam::Vec3) {
        let hi = position.as_vec3();
        let lo = (position - hi.as_dvec3()).as_vec3();
        self.position_hi_radius = [hi.x, hi.y, hi.z, self.position_hi_radius[3]];
        self.position_lo_strength = [lo.x, lo.y, lo.z, self.position_lo_strength[3]];
        self.velocity_wake = [velocity.x, velocity.y, velocity.z, self.velocity_wake[3]];
    }

    /// Its world position.
    pub fn position(&self) -> glam::DVec3 {
        let [hx, hy, hz, _] = self.position_hi_radius;
        let [lx, ly, lz, _] = self.position_lo_strength;
        glam::DVec3::new(
            f64::from(hx) + f64::from(lx),
            f64::from(hy) + f64::from(ly),
            f64::from(hz) + f64::from(lz),
        )
    }
}

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct FoliageWindRow {
    pub direction_speed: [f32; 4],
    pub gust: [f32; 4],
    pub time_prev_time: [f32; 2],
    pub _pad: [f32; 2],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for FoliageWindRow {}

/// The buffer the renderer reads [`FoliageWindRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const FOLIAGE_WIND: helio_core::BufferContract<FoliageWindRow> =
    helio_core::BufferContract::new("foliage_wind");

impl From<GpuFoliageType> for FoliageTypeRow {
    fn from(v: GpuFoliageType) -> Self {
        bytemuck::cast(v)
    }
}
impl From<FoliageTypeRow> for GpuFoliageType {
    fn from(v: FoliageTypeRow) -> Self {
        bytemuck::cast(v)
    }
}
impl From<GpuFoliageLayer> for FoliageLayerRow {
    fn from(v: GpuFoliageLayer) -> Self {
        bytemuck::cast(v)
    }
}
impl From<FoliageLayerRow> for GpuFoliageLayer {
    fn from(v: FoliageLayerRow) -> Self {
        bytemuck::cast(v)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;
    #[test]
    fn scene_records_preserve_gpu_abi() {
        assert_eq!(std::mem::size_of::<FoliageTypeRow>(), 128);
        assert_eq!(std::mem::size_of::<FoliageLayerRow>(), 32);
        assert_eq!(std::mem::size_of::<FoliageInteractorRow>(), 48);
        assert_eq!(std::mem::size_of::<FoliageWindRow>(), 48);
    }

    #[test]
    fn interactors_hold_planet_scale_positions() {
        let at = glam::DVec3::new(6_371_000.123_456, -1_234_567.891, 42.5);
        let row = FoliageInteractorRow::new(at, 1.0, 1.0, glam::Vec3::X, 1.5);
        assert!((row.position() - at).length() < 1.0e-6);
        assert_eq!(row.velocity_wake, [1.0, 0.0, 0.0, 1.5]);
    }
}
