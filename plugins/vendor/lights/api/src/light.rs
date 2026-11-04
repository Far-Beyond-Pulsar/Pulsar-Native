//! The light list's row: the GPU light ABI ([`GpuLight`]) and the SceneDB
//! row engines author it as ([`LightRow`], under [`SCENE_LIGHTS`]), and the
//! directional light published with it ([`DirectionalLightRow`]).
//!
//! Every pass that shades against lights (the lights' own lighting models
//! and shadow views, the meshes' forward and transparent drawers, the voxel
//! terrain, GI's tracers, the fog's shafts, the lens flare) reads the shape
//! from here; in WGSL, [`crate::LIGHT_LIST_SNIPPET`].

use pulsar_scenedb::gpu::world_mirror::DEFAULT_AUTO_REGISTER_CAPACITY;

/// The scene's lights: SceneDB rows ([`GpuLight`]), whatever kinds of light
/// are published (the lights' join, or a standalone host's rows).
pub const LIGHT_LIST: &str = "scene_lights";

/// Initial capacity of the light list ([`SCENE_LIGHTS`]): `World`'s own
/// auto-registration capacity, and the capacity frontends register it with.
/// The buffer grows past it; passes iterate its live
/// `BufferHandle::row_capacity()`, never this constant.
pub const MAX_LIGHTS: u32 = DEFAULT_AUTO_REGISTER_CAPACITY;

/// The lights' directional light, published with the light list: one
/// [`DirectionalLightRow`] (zero, or absent: no directional light). It is
/// the sun of whatever reads one (water, media, GI without an atmosphere):
/// the first enabled directional light with intensity, chosen once by the
/// lights, so no reader scans the list for it.
pub const DIRECTIONAL_LIGHT: &str = "directional_light";

/// [`DIRECTIONAL_LIGHT`]'s row. In WGSL two `vec4<f32>`: `[0]` toward the
/// light and 1, `[1]` its radiance (colour times intensity) and 0.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DirectionalLightRow {
    /// Unit direction toward the light (xyz), 1 when there is one.
    pub toward: [f32; 4],
    /// Linear RGB colour times intensity (xyz).
    pub radiance: [f32; 4],
}

use bytemuck::{Pod, Zeroable};

/// GPU light type discriminant.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightType {
    Directional = 0,
    Point = 1,
    Spot = 2,
    Area = 3,
}

/// Per-light GPU data. 96 bytes.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuLight {
///     position_range:    vec4<f32>,  // xyz = position, w = range
///     direction_outer:   vec4<f32>,  // xyz = direction, w = spot outer angle cos
///     color_intensity:   vec4<f32>,  // xyz = linear RGB, w = intensity
///     shadow_index:      u32,        // -1 if no shadow
///     light_type:        u32,        // LightType enum
///     inner_angle:       f32,        // spot inner angle cos
///     _pad:              u32,
///     god_rays_enabled:  u32,
///     god_rays_density:  f32,
///     god_rays_weight:   f32,
///     god_rays_decay:    f32,
///     god_rays_exposure: f32,
///     _pad2_0:           u32,
///     _pad2_1:           u32,
///     _pad2_2:           u32,
/// }
/// ```
///
/// The tail padding is three scalars, not a `vec3<u32>`: a WGSL `vec3` has
/// 16-byte alignment, so it would be pushed from offset 84 to 96 and grow the
/// struct to 112 — silently mismatching the 96-byte Rust side.
///
/// # Layout contract
///
/// This struct is mirrored **by hand** in every WGSL shader that binds the scene
/// light storage buffer. A field added here without updating each mirror silently
/// misreads the buffer — no validation error, just wrong lighting. Current mirrors:
///
/// - `helio-pass-deferred-light/shaders/deferred_lighting.wgsl`
/// - `helio-pass-light-cull/shaders/light_cull.wgsl`
/// - `helio-pass-hlfs/shaders/hlfs_shade.wgsl`
/// - `helio-pass-hlfs/shaders/hlfs_importance.wgsl`
/// - `helio-pass-shadow-matrix/shaders/shadow_matrices.wgsl`

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuLight {
    /// World-space position (xyz) + effective range (w)
    pub position_range: [f32; 4],
    /// Direction (xyz, normalized) + spot outer cos angle (w)
    pub direction_outer: [f32; 4],
    /// Linear RGB color (xyz) + intensity (w, in candela for point/spot, lux for directional)
    pub color_intensity: [f32; 4],
    /// As authored: `u32::MAX` for no shadow map, anything else (0 by
    /// convention) to request one. On the GPU, `ShadowMatrixPass`'s caster
    /// allocation replaces it with the assigned first atlas face (`6 × slot`)
    /// or `u32::MAX` if the light lost the budget (Helio#246).
    pub shadow_index: u32,
    /// LightType discriminant
    pub light_type: u32,
    /// Spot inner cos angle
    pub inner_angle: f32,
    /// Shadow-intent flags (name retained for source/ABI compatibility).
    /// Bit 0 marks explicit RT intent; bit 1 enables RT shadows. When bit 0 is
    /// clear, legacy lights derive intent from shadow_index before atlas admission.
    /// Bits 2-3 belong to the GPU caster allocation: bit 3 marks a row it has
    /// rewritten, bit 2 the shadow-map request it replaced. Authors leave
    /// them clear; a SceneDB re-upload of the row clears them.
    pub _pad: u32,

    // ── Light shafts / god rays (volumetric fog pass) ──
    /// Non-zero to accumulate light shafts for this light in the volumetric fog pass.
    pub god_rays_enabled: u32,
    /// Participating-media density along the shaft, independent of scene fog density.
    pub god_rays_density: f32,
    /// Per-step in-scattering weight.
    pub god_rays_weight: f32,
    /// Per-step attenuation (< 1.0 shortens the shaft).
    pub god_rays_decay: f32,
    /// Final scale applied to the accumulated shaft radiance.
    pub god_rays_exposure: f32,
    // ── Lens flare ──
    /// Non-zero to enable lens flare for this light.
    pub flare_enabled: u32,
    /// 0=none, 1=ghost, 2=anamorphic, 3=simple
    pub flare_type: u32,
    /// Overall intensity multiplier for the flare.
    pub flare_intensity: f32,
    /// Overall size multiplier for the flare.
    pub flare_scale: f32,
    /// Flare tint (RGB).
    pub flare_tint_r: f32,
    pub flare_tint_g: f32,
    pub flare_tint_b: f32,
    // ── IES light profiles / light functions ──
    pub ies_profile_index: i32, // -1 = no IES profile, 0+ = index into IES texture array
    pub light_function_index: i32, // -1 = no gobo, 0+ = index into light function texture array
    pub ies_angle_scale: f32,   // Angular scale for the IES profile
    pub ies_angle_offset: f32,  // Angular rotation offset for the IES profile (degrees)
}

// The WGSL mirrors above assume this exact size. A storage-buffer array of
// GpuLight strides by size_of::<GpuLight>(), so any drift shifts every light
// after index 0.
const _: () = assert!(std::mem::size_of::<GpuLight>() == 128);
// WGSL rounds the array stride up to the struct's alignment, which is 16 here
// (vec4<f32> members). If the Rust size were not a multiple of 16 the two sides
// would stride differently even at identical field counts.
const _: () = assert!(std::mem::size_of::<GpuLight>() % 16 == 0);

impl GpuLight {
    /// Encode author policy without changing the 128-byte GPU light ABI.
    /// Priority is quantized in sixteenths; zero encoding means the default 1.
    pub fn shadow_policy_bits(
        priority: f32,
        max_resolution: u32,
        static_shadows: bool,
        dynamic_shadows: bool,
        contact_shadows: bool,
    ) -> u32 {
        let priority = if priority.is_finite() {
            (priority.clamp(0.0625, 15.9375) * 16.0).round() as u32
        } else {
            16
        };
        let resolution = max_resolution.clamp(128, 2048).ilog2();
        (u32::from(!static_shadows) << 4)
            | (u32::from(!dynamic_shadows) << 5)
            | (u32::from(!contact_shadows) << 6)
            | (priority << 8)
            | (resolution << 16)
    }

    pub fn set_shadow_policy(
        &mut self,
        priority: f32,
        max_resolution: u32,
        static_shadows: bool,
        dynamic_shadows: bool,
        contact_shadows: bool,
    ) {
        self._pad = (self._pad & 15)
            | Self::shadow_policy_bits(
                priority,
                max_resolution,
                static_shadows,
                dynamic_shadows,
                contact_shadows,
            );
    }

    /// Set ray-traced shadow intent independently of shadow-map allocation.
    /// External GPU light writers must encode the same low two bits in `_pad`.
    pub fn set_ray_traced_shadows(&mut self, enabled: bool) {
        self._pad = (self._pad & !3) | 1 | (u32::from(enabled) << 1);
    }

    pub fn ray_traced_shadows(&self) -> bool {
        if self._pad & 1 != 0 {
            self._pad & 2 != 0
        } else {
            self.shadow_index != u32::MAX
        }
    }
}

impl Default for GpuLight {
    fn default() -> Self {
        Self {
            position_range: [0.0, 0.0, 0.0, 0.0],
            direction_outer: [0.0, -1.0, 0.0, 0.0],
            color_intensity: [1.0, 1.0, 1.0, 1.0],
            shadow_index: u32::MAX,
            light_type: LightType::Point as u32,
            inner_angle: 0.0,
            _pad: 0,

            // Off by default, but with usable values behind the switch: the fog
            // pass multiplies by density, weight and exposure, so leaving those at
            // 0.0 would make `god_rays_enabled = 1` render nothing and look broken.
            god_rays_enabled: 0,
            god_rays_density: 1.0,
            god_rays_weight: 0.6,
            god_rays_decay: 1.0,
            god_rays_exposure: 0.7,
            // Off by default — only explicitly marked lights produce flares.
            flare_enabled: 0,
            flare_type: 0,
            flare_intensity: 1.0,
            flare_scale: 1.0,
            flare_tint_r: 1.0,
            flare_tint_g: 1.0,
            flare_tint_b: 1.0,
            ies_profile_index: -1,
            light_function_index: -1,
            ies_angle_scale: 1.0,
            ies_angle_offset: 0.0,
        }
    }
}
#[cfg(test)]
mod ray_shadow_tests {
    use super::*;
    #[test]
    fn explicit_shadow_intent_survives_atlas_assignment() {
        let mut light = GpuLight::default();
        light.shadow_index = 0;
        assert!(light.ray_traced_shadows());
        light.set_ray_traced_shadows(true);
        light.shadow_index = u32::MAX;
        assert!(light.ray_traced_shadows());
        light.set_ray_traced_shadows(false);
        light.shadow_index = 0;
        assert!(!light.ray_traced_shadows());
        assert_eq!(std::mem::size_of::<GpuLight>(), 128);
    }
}

// ── The authored row ──────────────────────────────────────────────────────

/// A placed light, authored as a SceneDB component. Field-for-field
/// identical to [`GpuLight`] (enforced by the size/align test
/// below) so conversion is a zero-cost `bytemuck::cast`, matching
/// `CoronaEmitterRow`'s relationship to `GpuCoronaEmitter`.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct LightRow {
    pub position_range: [f32; 4],
    pub direction_outer: [f32; 4],
    pub color_intensity: [f32; 4],
    pub shadow_index: u32,
    pub light_type: u32,
    pub inner_angle: f32,
    pub _pad: u32,
    pub god_rays_enabled: u32,
    pub god_rays_density: f32,
    pub god_rays_weight: f32,
    pub god_rays_decay: f32,
    pub god_rays_exposure: f32,
    pub flare_enabled: u32,
    pub flare_type: u32,
    pub flare_intensity: f32,
    pub flare_scale: f32,
    pub flare_tint_r: f32,
    pub flare_tint_g: f32,
    pub flare_tint_b: f32,
    pub ies_profile_index: i32,
    pub light_function_index: i32,
    pub ies_angle_scale: f32,
    pub ies_angle_offset: f32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for LightRow {}

/// The buffer the renderer reads [`LightRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const SCENE_LIGHTS: helio_core::BufferContract<LightRow> =
    helio_core::BufferContract::new(LIGHT_LIST);

impl From<GpuLight> for LightRow {
    fn from(value: GpuLight) -> Self {
        bytemuck::cast(value)
    }
}

impl From<LightRow> for GpuLight {
    fn from(value: LightRow) -> Self {
        bytemuck::cast(value)
    }
}

#[cfg(test)]
mod row_tests {
    use super::{GpuLight, LightRow};

    #[test]
    fn scene_record_matches_light_gpu_abi() {
        assert_eq!(
            std::mem::size_of::<LightRow>(),
            std::mem::size_of::<GpuLight>()
        );
        assert_eq!(
            std::mem::align_of::<LightRow>(),
            std::mem::align_of::<GpuLight>()
        );
    }
}
