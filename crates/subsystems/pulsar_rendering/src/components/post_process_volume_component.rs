//! Post-process volume component (Phase D, Pulsar-Native#558) — the
//! largest of the "no purpose-built component exists yet" primitives:
//! exposure, bloom, color grading, white balance, tonemapping, vignette,
//! chromatic aberration, film grain, depth of field, motion blur, HDR
//! output, and volumetric fog, all blended per-volume by Helio's own
//! `PostProcessBlender`.
//!
//! Helio already has full native support for this
//! (`Scene::insert_post_process_volume`/`update_post_process_volume`/
//! `remove_post_process_volume`, `PostProcessVolumeDescriptor`,
//! `PostProcessSettings`) — same shape as `ReflectionCaptureComponent`/
//! `WaterVolumeComponent`, the gap this closes is purely the author-facing
//! `#[engine_class]` wrapper.
//!
//! Like `WaterVolumeComponent`, `bounds_min`/`bounds_max` are derived from
//! the owning object's position + an authored `size` rather than exposed as
//! raw absolute coordinates, so moving the object moves the volume.
//!
//! Two `PostProcessSettings` fields are deliberately not exposed:
//! `lut_generation`/`lut_platform`. Unlike every other field here, these
//! have no backing Rust enum and default to a bare `0` with no author-facing
//! meaning documented anywhere in `libhelio::postprocess` — internal
//! bookkeeping, not something a level designer tunes. Always written as `0`
//! when building the descriptor. `lut_intensity` (how strongly a baked LUT
//! applies) is kept — that one is genuinely author-tunable.

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use helio::{HdrOutputMode as HelioHdrOutputMode, Renderer, TonemapOperator as HelioTonemapOperator};
use libhelio::postprocess::{
    ExposureMode as HelioExposureMode, FogMode as HelioFogMode, PostProcessSettings,
    PostProcessVolumeDescriptor,
};
use pulsar_reflection::{
    get_subsystem, ComponentRuntimeBehavior, ComponentRuntimeContext, Reflectable,
    RuntimeComponentOwner,
};
use serde::{Deserialize, Serialize};

use crate::subsystems::PostProcessVolumeCache;

pub const POST_PROCESS_VOLUME_CLASS_NAME: &str = "PostProcessVolumeComponent";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum ExposureMode {
    Manual,
    Auto,
}
impl Default for ExposureMode {
    fn default() -> Self {
        Self::Manual
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum TonemapOperator {
    /// Skip tonemapping entirely.
    None,
    Aces,
    Filmic,
    Reinhard,
    Uncharted2,
    Lottes,
}
impl Default for TonemapOperator {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum FogMode {
    /// Constant density everywhere inside the fog region.
    Uniform,
    /// Density decays exponentially with world height above `fog_height`.
    HeightBased,
}
impl Default for FogMode {
    fn default() -> Self {
        Self::Uniform
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum HdrOutputMode {
    /// Tonemap → sRGB (standard display).
    Ldr,
    /// Tonemap → PQ ST 2084 → BT.2020 → 10-bit.
    Hdr10,
    /// Linear float output (scRGB, Windows HDR).
    ScRgb,
    /// Raw HDR float, no tonemap (external grading/recording).
    Passthrough,
}
impl Default for HdrOutputMode {
    fn default() -> Self {
        Self::Ldr
    }
}

/// A post-process settings volume, centered on the owning object.
#[engine_class(category = "Rendering", clone, debug, serialize, deserialize)]
#[category("Volume", category_color = "#8F8F8F")]
#[category("Exposure", category_color = "#D1A73F")]
#[category("Bloom", category_color = "#D1A73F")]
#[category("Color Grading", category_color = "#2FA88A")]
#[category("White Balance", category_color = "#2FA88A")]
#[category("Tonemap", category_color = "#2FA88A")]
#[category("Vignette", category_color = "#7C6FD1")]
#[category("Chromatic Aberration", category_color = "#7C6FD1")]
#[category("Film Grain", category_color = "#7C6FD1")]
#[category("Depth of Field", category_color = "#3AA0FF")]
#[category("Motion Blur", category_color = "#3AA0FF")]
#[category("Blend Weights", category_color = "#8F8F8F")]
#[category("HDR Output", category_color = "#D18F6F")]
#[category("Volumetric Fog", category_color = "#D18F6F")]
#[category("Advanced Color Grading", category_color = "#2FA88A")]
pub struct PostProcessVolumeComponent {
    #[property]
    pub enabled: bool,

    // ── Volume ──────────────────────────────────────────────────────────
    /// Full AABB extent (not half-extent) in each axis, centered on the
    /// owning object's position. Ignored when `unbound` is set.
    #[property(category = "Volume")]
    pub size: [f32; 3],
    /// Blend priority — higher-priority overlapping volumes win.
    #[property(category = "Volume")]
    pub priority: f32,
    /// Distance over which this volume's influence fades at its bounds.
    #[property(min = 0.0, max = 10000.0, step = 1.0, category = "Volume")]
    pub blend_radius: f32,
    /// Overall blend weight for this volume (0 = no effect, 1 = full).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Volume")]
    pub blend_weight: f32,
    /// Infinite volume — always active regardless of camera position (a
    /// global/default volume, typically one per level).
    #[property(category = "Volume")]
    pub unbound: bool,

    // ── Exposure ────────────────────────────────────────────────────────
    #[property(category = "Exposure")]
    pub exposure_mode: ExposureMode,
    #[property(min = -8.0, max = 8.0, step = 0.05, category = "Exposure")]
    pub exposure_compensation: f32,
    #[property(min = -16.0, max = 16.0, step = 0.05, category = "Exposure")]
    pub exposure_min: f32,
    #[property(min = -16.0, max = 16.0, step = 0.05, category = "Exposure")]
    pub exposure_max: f32,
    /// Seconds to adapt from dark to bright.
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Exposure")]
    pub exposure_speed_up: f32,
    /// Seconds to adapt from bright to dark.
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Exposure")]
    pub exposure_speed_down: f32,

    // ── Bloom ───────────────────────────────────────────────────────────
    #[property(category = "Bloom")]
    pub bloom_enabled: bool,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Bloom")]
    pub bloom_intensity: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Bloom")]
    pub bloom_threshold: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Bloom")]
    pub bloom_knee: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Bloom")]
    pub bloom_radius: f32,
    #[property(category = "Bloom")]
    pub bloom_tint: [f32; 3],

    // ── Color Grading ───────────────────────────────────────────────────
    #[property(category = "Color Grading")]
    pub color_saturation: [f32; 3],
    #[property(category = "Color Grading")]
    pub color_contrast: [f32; 3],
    #[property(category = "Color Grading")]
    pub color_gamma: [f32; 3],
    #[property(category = "Color Grading")]
    pub color_gain: [f32; 3],
    #[property(category = "Color Grading")]
    pub color_offset: [f32; 3],

    // ── White Balance ───────────────────────────────────────────────────
    #[property(category = "White Balance")]
    pub white_balance_enabled: bool,
    /// Color temperature, kelvin.
    #[property(min = 1000.0, max = 40000.0, step = 10.0, category = "White Balance")]
    pub white_temp: f32,
    #[property(min = -1.0, max = 1.0, step = 0.01, category = "White Balance")]
    pub white_tint: f32,

    // ── Tonemap ─────────────────────────────────────────────────────────
    #[property(category = "Tonemap")]
    pub tonemap_operator: TonemapOperator,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Tonemap")]
    pub tonemap_exposure: f32,
    #[property(min = 0.0, max = 20.0, step = 0.05, category = "Tonemap")]
    pub tonemap_white_point: f32,

    // ── Vignette ────────────────────────────────────────────────────────
    #[property(category = "Vignette")]
    pub vignette_enabled: bool,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Vignette")]
    pub vignette_intensity: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Vignette")]
    pub vignette_smoothness: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Vignette")]
    pub vignette_roundness: f32,
    #[property(category = "Vignette")]
    pub vignette_color: [f32; 3],

    // ── Chromatic Aberration ────────────────────────────────────────────
    #[property(category = "Chromatic Aberration")]
    pub ca_enabled: bool,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Chromatic Aberration")]
    pub ca_intensity: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Chromatic Aberration")]
    pub ca_start_offset: f32,

    // ── Film Grain ──────────────────────────────────────────────────────
    #[property(category = "Film Grain")]
    pub grain_enabled: bool,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Film Grain")]
    pub grain_intensity: f32,
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Film Grain")]
    pub grain_response: f32,
    #[property(min = 0.1, max = 10.0, step = 0.05, category = "Film Grain")]
    pub grain_size: f32,

    // ── Depth of Field ──────────────────────────────────────────────────
    #[property(category = "Depth of Field")]
    pub dof_enabled: bool,
    #[property(min = 0.0, max = 10000.0, step = 1.0, category = "Depth of Field")]
    pub dof_focal_distance: f32,
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Depth of Field")]
    pub dof_focal_region: f32,
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Depth of Field")]
    pub dof_near_transition: f32,
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Depth of Field")]
    pub dof_far_transition: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Depth of Field")]
    pub dof_scale: f32,
    #[property(min = 0.0, max = 100.0, step = 0.5, category = "Depth of Field")]
    pub dof_max_bokeh_size: f32,
    // `u32` isn't `Reflectable` (only `i32`/`i64`/`u64` are) -- `i32`,
    // clamped non-negative when building the descriptor, same fix as
    // `WaterVolumeComponent::ssr_steps`.
    #[property(min = 3, max = 16, category = "Depth of Field")]
    pub dof_aperture_blades: i32,
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Depth of Field")]
    pub dof_aperture_rotation: f32,
    #[property(min = 1.0, max = 100.0, step = 0.1, category = "Depth of Field")]
    pub dof_sensor_diagonal: f32,

    // ── Motion Blur ─────────────────────────────────────────────────────
    #[property(category = "Motion Blur")]
    pub motion_blur_enabled: bool,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Motion Blur")]
    pub motion_blur_amount: f32,
    #[property(min = 0.0, max = 256.0, step = 1.0, category = "Motion Blur")]
    pub motion_blur_max: f32,

    // ── Blend Weights ───────────────────────────────────────────────────
    // Per-effect blend weights, independent of the volume's own overall
    // `blend_weight` -- lets one effect fade in/out on its own transition
    // curve without touching the others.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_bloom: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_dof: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_motion_blur: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_vignette: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_ca: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_grain: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_exposure: f32,

    // ── HDR Output ──────────────────────────────────────────────────────
    #[property(category = "HDR Output")]
    pub hdr_output_mode: HdrOutputMode,
    #[property(min = 100.0, max = 10000.0, step = 10.0, category = "HDR Output")]
    pub hdr_max_nits: f32,
    #[property(min = 0.0, max = 1000.0, step = 1.0, category = "HDR Output")]
    pub hdr_ui_brightness: f32,

    // ── Volumetric Fog ──────────────────────────────────────────────────
    #[property(category = "Volumetric Fog")]
    pub fog_enabled: bool,
    #[property(category = "Volumetric Fog")]
    pub fog_mode: FogMode,
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Volumetric Fog")]
    pub fog_density: f32,
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Volumetric Fog")]
    pub fog_height_falloff: f32,
    #[property(min = 0.0, max = 100000.0, step = 10.0, category = "Volumetric Fog")]
    pub fog_start_distance: f32,
    #[property(min = 0.0, max = 100000.0, step = 10.0, category = "Volumetric Fog")]
    pub fog_max_distance: f32,
    #[property(category = "Volumetric Fog")]
    pub fog_height: f32,
    /// Henyey-Greenstein g: 0 = isotropic, >0 forward-scattering (sun haze),
    /// <0 back-scattering.
    #[property(min = -0.999, max = 0.999, step = 0.001, category = "Volumetric Fog")]
    pub fog_scattering_anisotropy: f32,
    #[property(category = "Volumetric Fog")]
    pub fog_color: [f32; 3],
    /// Self-illumination, added independently of any light (lava glow, etc.).
    #[property(category = "Volumetric Fog")]
    pub fog_emissive: [f32; 3],

    // ── Advanced Color Grading ──────────────────────────────────────────
    #[property(category = "Advanced Color Grading")]
    pub lift_color: [f32; 3],
    #[property(category = "Advanced Color Grading")]
    pub gamma_color: [f32; 3],
    #[property(category = "Advanced Color Grading")]
    pub gain_color: [f32; 3],
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Advanced Color Grading")]
    pub shadows_max: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Advanced Color Grading")]
    pub highlights_min: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Advanced Color Grading")]
    pub shadow_highlight_balance: f32,
    #[property(min = -180.0, max = 180.0, step = 1.0, category = "Advanced Color Grading")]
    pub hue_shift: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Advanced Color Grading")]
    pub lut_intensity: f32,
}

impl Default for PostProcessVolumeComponent {
    /// Mirrors `PostProcessSettings::default()`'s values, plus this
    /// component's own `size`/`priority`/`blend_radius`/`blend_weight`/
    /// `unbound` (mirroring `PostProcessVolumeDescriptor::default()`).
    fn default() -> Self {
        Self {
            enabled: true,
            size: [2000.0, 2000.0, 2000.0],
            priority: 0.0,
            blend_radius: 200.0,
            blend_weight: 1.0,
            unbound: false,
            exposure_mode: ExposureMode::Manual,
            exposure_compensation: 0.0,
            exposure_min: -4.0,
            exposure_max: 4.0,
            exposure_speed_up: 0.5,
            exposure_speed_down: 1.0,
            bloom_enabled: false,
            bloom_intensity: 0.3,
            bloom_threshold: 1.0,
            bloom_knee: 0.1,
            bloom_radius: 1.0,
            bloom_tint: [1.0, 1.0, 1.0],
            color_saturation: [1.0, 1.0, 1.0],
            color_contrast: [1.0, 1.0, 1.0],
            color_gamma: [1.0, 1.0, 1.0],
            color_gain: [1.0, 1.0, 1.0],
            color_offset: [0.0, 0.0, 0.0],
            white_balance_enabled: false,
            white_temp: 6500.0,
            white_tint: 0.0,
            tonemap_operator: TonemapOperator::None,
            tonemap_exposure: 1.0,
            tonemap_white_point: 1.0,
            vignette_enabled: false,
            vignette_intensity: 0.0,
            vignette_smoothness: 0.5,
            vignette_roundness: 0.5,
            vignette_color: [0.0, 0.0, 0.0],
            ca_enabled: false,
            ca_intensity: 0.0,
            ca_start_offset: 0.0,
            grain_enabled: false,
            grain_intensity: 0.0,
            grain_response: 1.0,
            grain_size: 1.0,
            dof_enabled: false,
            dof_focal_distance: 100.0,
            dof_focal_region: 50.0,
            dof_near_transition: 100.0,
            dof_far_transition: 100.0,
            dof_scale: 1.0,
            dof_max_bokeh_size: 10.0,
            dof_aperture_blades: 5,
            dof_aperture_rotation: 0.0,
            dof_sensor_diagonal: 43.3,
            motion_blur_enabled: false,
            motion_blur_amount: 0.0,
            motion_blur_max: 64.0,
            blend_weight_bloom: 1.0,
            blend_weight_dof: 1.0,
            blend_weight_motion_blur: 1.0,
            blend_weight_vignette: 1.0,
            blend_weight_ca: 1.0,
            blend_weight_grain: 1.0,
            blend_weight_exposure: 1.0,
            hdr_output_mode: HdrOutputMode::Ldr,
            hdr_max_nits: 1000.0,
            hdr_ui_brightness: 200.0,
            fog_enabled: false,
            fog_mode: FogMode::Uniform,
            fog_density: 0.02,
            fog_height_falloff: 0.05,
            fog_start_distance: 0.0,
            fog_max_distance: 1000.0,
            fog_height: 0.0,
            fog_scattering_anisotropy: 0.0,
            fog_color: [0.5, 0.6, 0.7],
            fog_emissive: [0.0, 0.0, 0.0],
            lift_color: [0.0; 3],
            gamma_color: [0.0; 3],
            gain_color: [1.0; 3],
            shadows_max: 0.3,
            highlights_min: 0.7,
            shadow_highlight_balance: 0.5,
            hue_shift: 0.0,
            lut_intensity: 1.0,
        }
    }
}

impl PostProcessVolumeComponent {
    fn to_descriptor(&self, owner: &RuntimeComponentOwner) -> PostProcessVolumeDescriptor {
        let [cx, cy, cz] = owner.position;
        let [sx, sy, sz] = self.size;
        let settings = PostProcessSettings {
            exposure_mode: match self.exposure_mode {
                ExposureMode::Manual => HelioExposureMode::Manual,
                ExposureMode::Auto => HelioExposureMode::Auto,
            },
            exposure_compensation: self.exposure_compensation,
            exposure_min: self.exposure_min,
            exposure_max: self.exposure_max,
            exposure_speed_up: self.exposure_speed_up,
            exposure_speed_down: self.exposure_speed_down,
            bloom_intensity: self.bloom_intensity,
            bloom_threshold: self.bloom_threshold,
            bloom_knee: self.bloom_knee,
            bloom_radius: self.bloom_radius,
            bloom_tint: self.bloom_tint,
            bloom_enabled: self.bloom_enabled,
            color_saturation: self.color_saturation,
            color_contrast: self.color_contrast,
            color_gamma: self.color_gamma,
            color_gain: self.color_gain,
            color_offset: self.color_offset,
            white_temp: self.white_temp,
            white_tint: self.white_tint,
            white_balance_enabled: self.white_balance_enabled,
            tonemap_operator: match self.tonemap_operator {
                TonemapOperator::None => HelioTonemapOperator::None,
                TonemapOperator::Aces => HelioTonemapOperator::Aces,
                TonemapOperator::Filmic => HelioTonemapOperator::Filmic,
                TonemapOperator::Reinhard => HelioTonemapOperator::Reinhard,
                TonemapOperator::Uncharted2 => HelioTonemapOperator::Uncharted2,
                TonemapOperator::Lottes => HelioTonemapOperator::Lottes,
            },
            tonemap_exposure: self.tonemap_exposure,
            tonemap_white_point: self.tonemap_white_point,
            vignette_intensity: self.vignette_intensity,
            vignette_smoothness: self.vignette_smoothness,
            vignette_roundness: self.vignette_roundness,
            vignette_color: self.vignette_color,
            vignette_enabled: self.vignette_enabled,
            ca_intensity: self.ca_intensity,
            ca_start_offset: self.ca_start_offset,
            ca_enabled: self.ca_enabled,
            grain_intensity: self.grain_intensity,
            grain_response: self.grain_response,
            grain_size: self.grain_size,
            grain_enabled: self.grain_enabled,
            dof_focal_distance: self.dof_focal_distance,
            dof_focal_region: self.dof_focal_region,
            dof_near_transition: self.dof_near_transition,
            dof_far_transition: self.dof_far_transition,
            dof_scale: self.dof_scale,
            dof_max_bokeh_size: self.dof_max_bokeh_size,
            dof_aperture_blades: self.dof_aperture_blades.max(0) as u32,
            dof_aperture_rotation: self.dof_aperture_rotation,
            dof_sensor_diagonal: self.dof_sensor_diagonal,
            dof_enabled: self.dof_enabled,
            motion_blur_amount: self.motion_blur_amount,
            motion_blur_max: self.motion_blur_max,
            motion_blur_enabled: self.motion_blur_enabled,
            blend_weight_bloom: self.blend_weight_bloom,
            blend_weight_dof: self.blend_weight_dof,
            blend_weight_motion_blur: self.blend_weight_motion_blur,
            blend_weight_vignette: self.blend_weight_vignette,
            blend_weight_ca: self.blend_weight_ca,
            blend_weight_grain: self.blend_weight_grain,
            blend_weight_exposure: self.blend_weight_exposure,
            hdr_output_mode: match self.hdr_output_mode {
                HdrOutputMode::Ldr => HelioHdrOutputMode::Ldr,
                HdrOutputMode::Hdr10 => HelioHdrOutputMode::Hdr10,
                HdrOutputMode::ScRgb => HelioHdrOutputMode::ScRgb,
                HdrOutputMode::Passthrough => HelioHdrOutputMode::Passthrough,
            },
            hdr_max_nits: self.hdr_max_nits,
            hdr_ui_brightness: self.hdr_ui_brightness,
            fog_enabled: self.fog_enabled,
            fog_mode: match self.fog_mode {
                FogMode::Uniform => HelioFogMode::Uniform,
                FogMode::HeightBased => HelioFogMode::HeightBased,
            },
            fog_density: self.fog_density,
            fog_height_falloff: self.fog_height_falloff,
            fog_start_distance: self.fog_start_distance,
            fog_max_distance: self.fog_max_distance,
            fog_height: self.fog_height,
            fog_scattering_anisotropy: self.fog_scattering_anisotropy,
            fog_color: self.fog_color,
            fog_emissive: self.fog_emissive,
            lift_color: self.lift_color,
            gamma_color: self.gamma_color,
            gain_color: self.gain_color,
            shadows_max: self.shadows_max,
            highlights_min: self.highlights_min,
            shadow_highlight_balance: self.shadow_highlight_balance,
            hue_shift: self.hue_shift,
            // Internal bookkeeping, not author-facing -- see this module's
            // top doc.
            lut_generation: 0,
            lut_intensity: self.lut_intensity,
            lut_platform: 0,
        };

        PostProcessVolumeDescriptor {
            bounds_min: [cx - sx * 0.5, cy - sy * 0.5, cz - sz * 0.5],
            bounds_max: [cx + sx * 0.5, cy + sy * 0.5, cz + sz * 0.5],
            priority: self.priority,
            blend_radius: self.blend_radius,
            blend_weight: self.blend_weight,
            unbound: self.unbound,
            settings,
        }
    }
}

#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for PostProcessVolumeComponent {
    const CLASS_NAME: &'static str = POST_PROCESS_VOLUME_CLASS_NAME;

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let cached_id = get_subsystem!(context, PostProcessVolumeCache).get(owner.scene_object_id);

        if !component.enabled {
            if let Some(id) = cached_id {
                let removed = get_subsystem!(context, Renderer)
                    .scene_mut()
                    .remove_post_process_volume(id);
                if removed.is_ok() {
                    get_subsystem!(context, PostProcessVolumeCache).remove(owner.scene_object_id);
                }
            }
            return;
        }

        let descriptor = component.to_descriptor(owner);

        match cached_id {
            Some(id) => {
                let _ = get_subsystem!(context, Renderer)
                    .scene_mut()
                    .update_post_process_volume(id, descriptor);
            }
            None => {
                let inserted = get_subsystem!(context, Renderer)
                    .scene_mut()
                    .insert_post_process_volume(descriptor);
                if let Ok(id) = inserted {
                    get_subsystem!(context, PostProcessVolumeCache)
                        .insert(owner.scene_object_id.to_string(), id);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_subsystems::{Subsystem, SubsystemContext};
    use pulsar_reflection::{apply_runtime_behavior_for_class, Subsystems};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    struct TestRuntimeContext {
        project_root: PathBuf,
        subsystems: Subsystems,
        errors: Vec<String>,
    }

    impl ComponentRuntimeContext for TestRuntimeContext {
        fn subsystems_mut(&mut self) -> &mut Subsystems {
            &mut self.subsystems
        }
        fn project_root(&self) -> &Path {
            &self.project_root
        }
        fn report_error(&mut self, message: String) {
            self.errors.push(message);
        }
    }

    fn owner<'a>(props: &'a HashMap<String, serde_json::Value>) -> RuntimeComponentOwner<'a> {
        RuntimeComponentOwner {
            scene_object_id: "volume",
            position: [0.0, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            props,
        }
    }

    #[test]
    fn runtime_behavior_has_the_reflected_component_name() {
        assert_eq!(
            <PostProcessVolumeComponent as ComponentRuntimeBehavior>::CLASS_NAME,
            POST_PROCESS_VOLUME_CLASS_NAME
        );
    }

    #[test]
    fn to_descriptor_centers_bounds_on_owner_position_and_maps_settings() {
        let component = PostProcessVolumeComponent {
            size: [10.0, 20.0, 10.0],
            bloom_enabled: true,
            bloom_intensity: 0.75,
            tonemap_operator: TonemapOperator::Aces,
            ..Default::default()
        };
        let props = HashMap::new();
        let mut o = owner(&props);
        o.position = [1.0, 2.0, 3.0];

        let descriptor = component.to_descriptor(&o);

        assert_eq!(descriptor.bounds_min, [-4.0, -8.0, -2.0]);
        assert_eq!(descriptor.bounds_max, [6.0, 12.0, 8.0]);
        assert!(descriptor.settings.bloom_enabled);
        assert_eq!(descriptor.settings.bloom_intensity, 0.75);
        assert_eq!(descriptor.settings.tonemap_operator, HelioTonemapOperator::Aces);
        assert_eq!(descriptor.settings.lut_generation, 0);
        assert_eq!(descriptor.settings.lut_platform, 0);
    }

    #[test]
    fn disabling_a_never_inserted_volume_is_a_quiet_no_op() {
        let mut subsystems = Subsystems::new();
        subsystems.register(PostProcessVolumeCache::new());
        let mut context = TestRuntimeContext {
            project_root: PathBuf::from("."),
            subsystems,
            errors: Vec::new(),
        };
        let props = HashMap::new();
        let disabled = PostProcessVolumeComponent { enabled: false, ..Default::default() };

        assert!(apply_runtime_behavior_for_class(
            POST_PROCESS_VOLUME_CLASS_NAME,
            &owner(&props),
            0,
            &serde_json::to_value(disabled).unwrap(),
            &mut context,
        ));
        assert!(context.errors.is_empty());
    }
}
