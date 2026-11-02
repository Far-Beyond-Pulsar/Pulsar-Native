//! Reflected camera/volume post-process authoring. A volume reaches the
//! post-process resolve as its derived row ([`super::environment_rows`]),
//! placed by the graph's environment join with the owner's transform; camera
//! settings reuse PostProcessSettingsProps.

use super::lens_flare_props::LensFlareProps;
use engine_class_derive::{engine_class, register_world_component};
use helio_pass_postprocess::{
    ExposureMode as HelioExposureMode, FogMode as HelioFogMode, PostProcessSettings,
    PostProcessVolumeDescriptor,
};
use helio_pass_postprocess::{
    HdrOutputMode as HelioHdrOutputMode, TonemapOperator as HelioTonemapOperator,
};
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

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
    /// Hue-preserving filmic curve: bright saturated colours roll to white.
    Agx,
    /// AgX with its punchy look: more contrast and saturation.
    AgxPunchy,
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
    /// Animated world-space billows with height falloff.
    Smoke,
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
#[category("Local Exposure", category_color = "#D1A73F")]
#[category("Vignette", category_color = "#7C6FD1")]
#[category("Film Grain", category_color = "#7C6FD1")]
#[category("Motion Blur", category_color = "#3AA0FF")]
#[category("Blend Weights", category_color = "#8F8F8F")]
#[category("HDR Output", category_color = "#D18F6F")]
#[category("Volumetric Fog", category_color = "#D18F6F")]
#[category("Advanced Color Grading", category_color = "#2FA88A")]
#[category("Lens Flare", category_color = "#E0B040")]
#[category("Overrides", category_color = "#8F8F8F")]
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

    /// Which setting groups this volume overrides; the rest pass through
    /// from the camera baseline and lower-priority volumes untouched.
    #[sub_props]
    #[serde(flatten)]
    pub overrides: PostProcessOverrides,

    #[sub_props]
    #[serde(flatten)]
    pub settings: PostProcessSettingsProps,
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
            overrides: PostProcessOverrides::default(),
            settings: PostProcessSettingsProps::default(),
        }
    }
}

/// Per-group override switches, packed into `PostProcessProperty` bits.
///
/// Groups that predate override masks default on, so existing volumes keep
/// replacing them exactly as before. Lens flare and local exposure default
/// off: a volume placed for grading or fog must not silently replace the
/// camera's lens or local exposure.
#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[serde(default)]
#[category("Overrides", category_color = "#8F8F8F")]
pub struct PostProcessOverrides {
    #[property(category = "Overrides")]
    pub override_exposure: bool,
    #[property(category = "Overrides")]
    pub override_bloom: bool,
    /// Color grading, white balance and advanced grading.
    #[property(category = "Overrides")]
    pub override_color_grading: bool,
    #[property(category = "Overrides")]
    pub override_tonemap: bool,
    #[property(category = "Overrides")]
    pub override_local_exposure: bool,
    #[property(category = "Overrides")]
    pub override_vignette: bool,
    #[property(category = "Overrides")]
    pub override_film_grain: bool,
    #[property(category = "Overrides")]
    pub override_motion_blur: bool,
    #[property(category = "Overrides")]
    pub override_hdr_output: bool,
    /// Legacy PP fog. Prefer Global/Local fog components for world media.
    #[property(category = "Overrides")]
    pub override_volumetric_fog: bool,
    #[property(category = "Overrides")]
    pub override_lens_flare: bool,
}

impl Default for PostProcessOverrides {
    fn default() -> Self {
        Self {
            override_exposure: true,
            override_bloom: true,
            override_color_grading: true,
            override_tonemap: true,
            override_local_exposure: false,
            override_vignette: true,
            override_film_grain: true,
            override_motion_blur: true,
            override_hdr_output: true,
            override_volumetric_fog: true,
            override_lens_flare: false,
        }
    }
}

impl PostProcessOverrides {
    pub fn mask(&self) -> [u32; 4] {
        use helio_pass_postprocess::PostProcessProperty as P;
        let groups: [(bool, &[usize]); 11] = [
            (
                self.override_exposure,
                &[
                    P::EXPOSURE_MODE,
                    P::EXPOSURE_COMPENSATION,
                    P::EXPOSURE_MIN,
                    P::EXPOSURE_MAX,
                    P::EXPOSURE_SPEED_UP,
                    P::EXPOSURE_SPEED_DOWN,
                    P::BLEND_WEIGHT_EXPOSURE,
                ],
            ),
            (
                self.override_bloom,
                &[
                    P::BLOOM_INTENSITY,
                    P::BLOOM_THRESHOLD,
                    P::BLOOM_KNEE,
                    P::BLOOM_RADIUS,
                    P::BLOOM_TINT,
                    P::BLOOM_ENABLED,
                    P::BLEND_WEIGHT_BLOOM,
                ],
            ),
            (
                self.override_color_grading,
                &[
                    P::COLOR_SATURATION,
                    P::COLOR_CONTRAST,
                    P::COLOR_GAMMA,
                    P::COLOR_GAIN,
                    P::COLOR_OFFSET,
                    P::WHITE_TEMP,
                    P::WHITE_TINT,
                    P::WHITE_BALANCE_ENABLED,
                    P::LIFT_COLOR,
                    P::GAMMA_COLOR,
                    P::GAIN_COLOR,
                    P::SHADOWS_MAX,
                    P::HIGHLIGHTS_MIN,
                    P::SHADOW_HIGHLIGHT_BALANCE,
                    P::HUE_SHIFT,
                    P::LUT_GENERATION,
                    P::LUT_INTENSITY,
                    P::LUT_PLATFORM,
                ],
            ),
            (
                self.override_tonemap,
                &[
                    P::TONEMAP_OPERATOR,
                    P::TONEMAP_EXPOSURE,
                    P::TONEMAP_WHITE_POINT,
                ],
            ),
            (
                self.override_local_exposure,
                &[
                    P::LOCAL_EXPOSURE_HIGHLIGHT_CONTRAST,
                    P::LOCAL_EXPOSURE_SHADOW_CONTRAST,
                    P::LOCAL_EXPOSURE_DETAIL_STRENGTH,
                ],
            ),
            (
                self.override_vignette,
                &[
                    P::VIGNETTE_INTENSITY,
                    P::VIGNETTE_SMOOTHNESS,
                    P::VIGNETTE_ROUNDNESS,
                    P::VIGNETTE_COLOR,
                    P::VIGNETTE_ENABLED,
                    P::BLEND_WEIGHT_VIGNETTE,
                ],
            ),
            (
                self.override_film_grain,
                &[
                    P::GRAIN_INTENSITY,
                    P::GRAIN_RESPONSE,
                    P::GRAIN_SIZE,
                    P::GRAIN_ENABLED,
                    P::BLEND_WEIGHT_GRAIN,
                ],
            ),
            (
                self.override_motion_blur,
                &[
                    P::MOTION_BLUR_AMOUNT,
                    P::MOTION_BLUR_MAX,
                    P::MOTION_BLUR_ENABLED,
                    P::BLEND_WEIGHT_MOTION_BLUR,
                ],
            ),
            (
                self.override_hdr_output,
                &[P::HDR_OUTPUT_MODE, P::HDR_MAX_NITS, P::HDR_UI_BRIGHTNESS],
            ),
            (
                self.override_volumetric_fog,
                &[
                    P::FOG_ENABLED,
                    P::FOG_MODE,
                    P::FOG_DENSITY,
                    P::FOG_HEIGHT_FALLOFF,
                    P::FOG_START_DISTANCE,
                    P::FOG_MAX_DISTANCE,
                    P::FOG_HEIGHT,
                    P::FOG_SCATTERING_ANISOTROPY,
                    P::FOG_COLOR,
                    P::FOG_EMISSIVE,
                ],
            ),
            (
                self.override_lens_flare,
                &[
                    P::LENS_ENABLED,
                    P::LENS_QUALITY,
                    P::LENS_PROFILE,
                    P::LENS_GHOST_COUNT,
                    P::LENS_INTENSITY,
                    P::LENS_THRESHOLD,
                    P::LENS_SOFT_KNEE,
                    P::LENS_GHOST_INTENSITY,
                    P::LENS_HALO_INTENSITY,
                    P::LENS_GLARE_INTENSITY,
                    P::LENS_STREAK_INTENSITY,
                    P::LENS_DISPERSION,
                    P::LENS_STARBURST_INTENSITY,
                    P::LENS_STARBURST_LENGTH,
                    P::LENS_COATING_STRENGTH,
                    P::LENS_GHOST_RIM,
                    P::LENS_DIRT_INTENSITY,
                    P::LENS_LIGHT_SOURCES,
                    P::LENS_LIGHT_INTENSITY,
                    P::LENS_FIELD_MARGIN,
                    P::LENS_RESPONSE_TIME,
                ],
            ),
        ];
        let mut mask = P::NONE;
        for (enabled, properties) in groups {
            for &property in properties {
                P::set(&mut mask, property, enabled);
            }
        }
        mask
    }
}

/// Reflected controls shared by camera baselines and spatial PP volumes.
/// Flattened serialization preserves the existing volume scene format.
#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[serde(default)]
#[category("Exposure", category_color = "#D1A73F")]
#[category("Bloom", category_color = "#D1A73F")]
#[category("Color Grading", category_color = "#2FA88A")]
#[category("White Balance", category_color = "#2FA88A")]
#[category("Tonemap", category_color = "#2FA88A")]
#[category("Local Exposure", category_color = "#D1A73F")]
#[category("Vignette", category_color = "#7C6FD1")]
#[category("Film Grain", category_color = "#7C6FD1")]
#[category("Motion Blur", category_color = "#3AA0FF")]
#[category("Blend Weights", category_color = "#8F8F8F")]
#[category("HDR Output", category_color = "#D18F6F")]
#[category("Volumetric Fog", category_color = "#D18F6F")]
#[category("Advanced Color Grading", category_color = "#2FA88A")]
#[category("Lens Flare", category_color = "#E0B040")]
pub struct PostProcessSettingsProps {
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

    // ── Local Exposure ──────────────────────────────────────────────────
    // Edge-aware local tone mapping: each region's mean brightness moves
    // toward middle grey (shade reads without fill lights, bright areas keep
    // their exposure), without halos at silhouettes. 1 leaves a side as it is.
    /// Contrast kept by regions brighter than middle grey (1 = unchanged).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Local Exposure")]
    pub local_exposure_highlight_contrast: f32,
    /// Contrast kept by regions darker than middle grey (1 = unchanged,
    /// lower lifts shade further).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Local Exposure")]
    pub local_exposure_shadow_contrast: f32,
    /// Contrast within a region (1 = kept, above 1 sharpened).
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Local Exposure")]
    pub local_exposure_detail_strength: f32,

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

    // ── Film Grain ──────────────────────────────────────────────────────
    #[property(category = "Film Grain")]
    pub grain_enabled: bool,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Film Grain")]
    pub grain_intensity: f32,
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Film Grain")]
    pub grain_response: f32,
    #[property(min = 0.1, max = 10.0, step = 0.05, category = "Film Grain")]
    pub grain_size: f32,

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
    pub blend_weight_motion_blur: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Blend Weights")]
    pub blend_weight_vignette: f32,
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
    /// Camera optics, evaluated after medium compositing and exposure.
    #[property(category = "Lens Flare")]
    pub lens_flare: LensFlareProps,
}

impl Default for PostProcessSettingsProps {
    fn default() -> Self {
        Self {
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
            tonemap_operator: TonemapOperator::AgxPunchy,
            tonemap_exposure: 1.0,
            tonemap_white_point: 1.0,
            local_exposure_highlight_contrast: helio_pass_postprocess::LOCAL_EXPOSURE_HIGHLIGHT_CONTRAST,
            local_exposure_shadow_contrast: helio_pass_postprocess::LOCAL_EXPOSURE_SHADOW_CONTRAST,
            local_exposure_detail_strength: helio_pass_postprocess::LOCAL_EXPOSURE_DETAIL_STRENGTH,
            vignette_enabled: false,
            vignette_intensity: 0.0,
            vignette_smoothness: 0.5,
            vignette_roundness: 0.5,
            vignette_color: [0.0, 0.0, 0.0],
            grain_enabled: false,
            grain_intensity: 0.0,
            grain_response: 1.0,
            grain_size: 1.0,
            motion_blur_enabled: false,
            motion_blur_amount: 0.0,
            motion_blur_max: 64.0,
            blend_weight_bloom: 1.0,
            blend_weight_motion_blur: 1.0,
            blend_weight_vignette: 1.0,
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
            lens_flare: LensFlareProps::default(),
        }
    }
}

impl PostProcessSettingsProps {
    pub fn to_settings(&self) -> PostProcessSettings {
        PostProcessSettings {
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
                TonemapOperator::Agx => HelioTonemapOperator::Agx,
                TonemapOperator::AgxPunchy => HelioTonemapOperator::AgxPunchy,
            },
            tonemap_exposure: self.tonemap_exposure,
            tonemap_white_point: self.tonemap_white_point,
            local_exposure_highlight_contrast: self.local_exposure_highlight_contrast,
            local_exposure_shadow_contrast: self.local_exposure_shadow_contrast,
            local_exposure_detail_strength: self.local_exposure_detail_strength,
            vignette_intensity: self.vignette_intensity,
            vignette_smoothness: self.vignette_smoothness,
            vignette_roundness: self.vignette_roundness,
            vignette_color: self.vignette_color,
            vignette_enabled: self.vignette_enabled,
            grain_intensity: self.grain_intensity,
            grain_response: self.grain_response,
            grain_size: self.grain_size,
            grain_enabled: self.grain_enabled,
            motion_blur_amount: self.motion_blur_amount,
            motion_blur_max: self.motion_blur_max,
            motion_blur_enabled: self.motion_blur_enabled,
            blend_weight_bloom: self.blend_weight_bloom,
            blend_weight_motion_blur: self.blend_weight_motion_blur,
            blend_weight_vignette: self.blend_weight_vignette,
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
                FogMode::Smoke => HelioFogMode::Smoke,
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
            lens_flare: self.lens_flare.to_settings(),
        }
    }
}

impl PostProcessVolumeComponent {
    /// The volume as a pass descriptor centred on the origin, unscaled: the
    /// environment join places it with the owner's transform.
    pub fn local_descriptor(&self) -> PostProcessVolumeDescriptor {
        let [sx, sy, sz] = self.size;
        PostProcessVolumeDescriptor {
            bounds_min: [-sx * 0.5, -sy * 0.5, -sz * 0.5],
            bounds_max: [sx * 0.5, sy * 0.5, sz * 0.5],
            priority: self.priority,
            blend_radius: self.blend_radius,
            blend_weight: self.blend_weight,
            unbound: self.unbound,
            override_mask: self.overrides.mask(),
            settings: self.settings.to_settings(),
        }
    }
}

#[register_world_component]
impl PostProcessVolumeComponent {}

#[cfg(test)]
mod tests {
    use super::*;

    /// An authored component left at its defaults renders the engine's
    /// defaults: the editor and every host start from the same look Helio's
    /// own renderer does.
    #[test]
    fn default_props_are_the_engines_defaults() {
        assert_eq!(
            format!("{:?}", PostProcessSettingsProps::default().to_settings()),
            format!("{:?}", PostProcessSettings::default()),
        );
    }
}
