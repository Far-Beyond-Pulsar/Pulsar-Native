//! Water volume and hitbox GPU types.
//!
//! This module defines the GPU-side representations for:
//! - [`GpuWaterVolume`]: per-volume rendering parameters (waves, caustics, etc.)
//! - [`GpuWaterHitbox`]: per-frame AABB hitbox that displaces the water heightfield

use bytemuck::{Pod, Zeroable};

/// [`GpuWaterVolume`] in WGSL (`//!use water_volume`): `struct WaterVolume`,
/// field for field. Every water shader that reads the volumes' rows (the
/// surface, caustics, simulation update and hitboxes, spray, underwater fog
/// and shafts, transmission) takes the row from here and declares only its
/// own binding of it; so do the snippets that read a row
/// ([`crate::pipeline::WATER_SHORE_SNIPPET`],
/// [`crate::surface_query::WATER_QUERY_SNIPPET`]) through
/// the shaders that compose them.
pub const WATER_VOLUME_SNIPPET: helio_core::shader::ShaderSnippet =
    helio_core::wgsl_snippet!("//!use water_volume", "../shaders/water_volume.wgsl");

/// GPU water volume descriptor (256 bytes, 16-byte aligned).
///
/// Defines a water volume's bounds, wave parameters, visual properties,
/// and rendering settings. Stored in GPU storage buffers for efficient
/// access by water rendering shaders.
///
/// # Memory Layout
/// - Total size: 256 bytes (16 × vec4<f32>)
/// - Alignment: 16 bytes
///
/// # Field layout (slot → name → WGSL vec4 index)
/// 0  bounds_min              → vec4 containing (x,y,z,_)
/// 1  bounds_max              → vec4 containing (x,y,z,_)
/// 2  wave_params             → (amplitude, frequency, speed, steepness)
/// 3  wave_direction          → (dx, dz, _, _)
/// 4  water_color             → (r, g, b, Jacobian foam bias)
/// 5  extinction              → (r, g, b, Jacobian foam scale) [rgb absorption is Beer-Lambert]
/// 6  reflection_refraction   → (refl_strength, refr_strength, fresnel_power, foam brightness)
/// 7  caustics_params         → (enabled, intensity, scale, speed)
/// 8  fog_params              → (density, god_rays, _, _)
/// 9  sim_params              → (ior, caustic_intensity, fresnel_min, density)
/// 10 shadow_params           → (rim, hitbox_shadow, ao, _)
/// 11 sun_direction           → (dx, dy, dz, found)  [toward the sun]
/// 12 ssr_params              → (enable, max_steps, step_size, thickness)
/// 13 sim_dynamics            → (spring, damping, wave_scale, _)
/// 14 wind_params             → (dir_x, dir_z, strength, own_wind)
/// 15 world_phase             → (x, z, _, _)  [world origin mod WATER_WORLD_PERIOD]
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuWaterVolume {
    /// Minimum bounds (xyz) + padding
    pub bounds_min: [f32; 4],

    /// Maximum bounds (xyz) + surface height in w component
    pub bounds_max: [f32; 4],

    /// Wave parameters: (amplitude, frequency, speed, steepness)
    pub wave_params: [f32; 4],

    /// Wave direction (dx, dz) + padding
    pub wave_direction: [f32; 4],

    /// Water base color (rgb) + Jacobian foam bias (w; Ultra-Water default 0.05).
    pub water_color: [f32; 4],

    /// Color absorption per meter (rgb) + Jacobian foam scale (w; Ultra-Water default 1.3).
    /// `w` is independent of Beer-Lambert absorption and does not scale shoreline foam.
    pub extinction: [f32; 4],

    /// Reflection strength, refraction strength, fresnel power, foam brightness.
    /// A zero `w` retains compatibility with older rows and selects brightness 0.85.
    pub reflection_refraction: [f32; 4],

    /// Caustics: enabled (0/1), intensity, scale, speed
    pub caustics_params: [f32; 4],

    /// Fog density, god rays intensity, padding, padding
    pub fog_params: [f32; 4],

    /// Heightfield simulation surface params: (ior, caustic_intensity, fresnel_min, density)
    pub sim_params: [f32; 4],

    /// Shadow parameters: (rim_light, hitbox_shadow, ambient_occlusion, padding)
    pub shadow_params: [f32; 4],

    /// Direction toward the sun (dx, dy, dz) and 1 in `w` when the scene
    /// has one. Water's row join writes the lights' directional light here
    /// (`helio_frame_api::lighting::DIRECTIONAL_LIGHT`); hosts that write
    /// rows themselves author it.
    pub sun_direction: [f32; 4],

    /// SSR parameters: (enable 0/1, max_steps, step_size, thickness)
    pub ssr_params: [f32; 4],
    /// Simulation physics: (wave_spring, wave_damping, wave_scale, _).
    /// wave_spring: restoring-force multiplier. wave_damping: per-step energy
    /// retention. wave_scale: spatial scale of the wind's wave trains. The
    /// simulation reads this volume's dynamics, wind and wave speed
    /// (`wave_params.z`) from its row when `wave_spring` is positive, and
    /// the pass-wide values (`WaterSimPass::set_sim_dynamics`, `set_wind`,
    /// `set_wave_scale`, `set_wave_speed`) for a row that leaves them zero.
    pub sim_dynamics: [f32; 4],
    /// Wind parameters: (dir_x, dir_z, strength, own_wind). `own_wind` is 1
    /// when the volume opts out of the level's global wind, which the
    /// environment join otherwise writes over the first three.
    pub wind_params: [f32; 4],
    /// Where the row's frame sits in the waves' world tiling: the frame's
    /// world origin modulo [`crate::WATER_WORLD_PERIOD`] (`x`, `z`), zero
    /// for a world-space frame. The environment join writes it for
    /// camera-relative frames; hosts that write rows themselves leave it
    /// zero.
    pub world_phase: [f32; 4],
}

impl GpuWaterVolume {
    /// Ocean preset with heightfield sim defaults.
    pub fn default_ocean() -> Self {
        Self {
            bounds_min: [-100.0, -10.0, -100.0, 0.0],
            bounds_max: [100.0, 50.0, 100.0, 0.0],
            wave_params: [0.5, 0.3, 1.5, 0.5],
            wave_direction: [1.0, 0.0, 0.0, 0.0],
            water_color: [0.0, 0.2, 0.4, 0.05],
            extinction: [0.1, 0.05, 0.02, 1.3],
            reflection_refraction: [0.8, 0.2, 5.0, 0.85],
            caustics_params: [1.0, 1.5, 5.0, 0.5],
            fog_params: [0.03, 1.0, 0.0, 0.0],
            // ior=1.333, caustic_intensity=1.5, fresnel_min=0.02 (water's F0), density=0.03
            sim_params: [1.333, 1.5, 0.02, 0.03],
            // rim=1.0, hitbox_shadow=0.0, ao=1.0
            shadow_params: [1.0, 0.0, 1.0, 0.0],
            // normalized sun direction
            sun_direction: [0.408_248_3, 0.816_496_6, 0.408_248_3, 0.0],
            // SSR: enabled, 32 steps, 0.05 world-space step size, 0.02 thickness
            ssr_params: [1.0, 32.0, 0.05, 0.02],
            // wave_spring=1.2, wave_damping=0.985 — fluid, not jelly
            sim_dynamics: [1.2, 0.985, 1.0, 0.0],
            wind_params: [0.0; 4],
            world_phase: [0.0; 4],
        }
    }

    /// Lake/pool preset with heightfield sim defaults.
    pub fn default_lake() -> Self {
        Self {
            bounds_min: [-50.0, -5.0, -50.0, 0.0],
            bounds_max: [50.0, 20.0, 50.0, 0.0],
            wave_params: [0.2, 0.5, 0.8, 0.3],
            wave_direction: [1.0, 0.0, 0.0, 0.0],
            water_color: [0.1, 0.3, 0.2, 0.05],
            extinction: [0.2, 0.1, 0.08, 1.3],
            reflection_refraction: [0.6, 0.3, 4.0, 0.85],
            caustics_params: [1.0, 1.2, 4.0, 0.4],
            fog_params: [0.05, 0.5, 0.0, 0.0],
            sim_params: [1.333, 1.2, 0.1, 0.05],
            shadow_params: [1.0, 0.0, 1.0, 0.0],
            sun_direction: [0.408_248_3, 0.816_496_6, 0.408_248_3, 0.0],
            // SSR: enabled, 32 steps, 0.05 world-space step size, 0.02 thickness
            ssr_params: [1.0, 32.0, 0.05, 0.02],
            // wave_spring=1.0, wave_damping=0.980 — still water settles fast
            sim_dynamics: [1.0, 0.980, 1.0, 0.0],
            wind_params: [0.0; 4],
            world_phase: [0.0; 4],
        }
    }
}

// Compile-time size verification
const _: () = assert!(
    std::mem::size_of::<GpuWaterVolume>() == 256,
    "GpuWaterVolume must be exactly 256 bytes"
);

const _: () = assert!(
    std::mem::align_of::<GpuWaterVolume>() <= 16,
    "GpuWaterVolume alignment must be 16 bytes or less for GPU compatibility"
);

// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
// Water Hitbox
// ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

/// GPU-side AABB hitbox that displaces the water heightfield simulation.
///
/// Each hitbox records where an object *was* (old bounds) and where it *is* (new bounds),
/// world space. The simulation shader displaces water upward where the hitbox moved away and
/// downward where it moved to, producing realistic wave entry effects, in every water
/// volume the box overlaps. The pass remembers the rows it applied last: while a row
/// holds the same object (`params.z`), its old bounds are the ones it had then, so an
/// object at rest displaces nothing.
///
/// # Memory Layout
/// - Total size: 80 bytes (5 × vec4<f32>)
/// - Alignment: 16 bytes
///
/// # Algorithm
/// The displacement uses a smooth Gaussian-like falloff within the AABB:
/// - Rise = volume_in_box(old_center, old_half_extent, position)
/// - Fall = volume_in_box(new_center, new_half_extent, position)
/// - delta_height += strength * (rise - fall)
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuWaterHitbox {
    /// Previous frame AABB minimum (xyz) + padding
    pub old_min: [f32; 4],
    /// Previous frame AABB maximum (xyz) + padding
    pub old_max: [f32; 4],
    /// Current frame AABB minimum (xyz) + padding
    pub new_min: [f32; 4],
    /// Current frame AABB maximum (xyz) + padding
    pub new_max: [f32; 4],
    /// (edge_softness, strength, identity, padding)
    /// - edge_softness: how far, in metres, the falloff extends past the AABB edges
    /// - strength: displacement multiplier (default 1.0; 0 = inert row)
    /// - identity: which object the row holds (the environment join writes its
    ///   source row + 1); a change starts from this row's own old bounds
    pub params: [f32; 4],
}

const _: () = assert!(
    std::mem::size_of::<GpuWaterHitbox>() == 80,
    "GpuWaterHitbox must be exactly 80 bytes"
);
