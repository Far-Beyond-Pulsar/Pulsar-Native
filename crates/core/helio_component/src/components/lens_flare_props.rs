//! Reflected lens controls; field names match the pass-owned settings.
//!
//! Lens flare is camera optics: it lives in post-process settings (camera
//! baseline and PP volumes), never on lights. Every bright thing in frame
//! produces a response in proportion to its scene-linear radiance.
use engine_class_derive::engine_class;
use helio_pass_postprocess::LensFlareSettings;
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum LensQuality {
    /// 8 pupil taps, up to 4 ghosts. Same model and energy as High.
    Economical,
    /// 24 pupil taps, up to 8 ghosts.
    High,
}
impl Default for LensQuality {
    fn default() -> Self {
        Self::Economical
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum LensProfile {
    Spherical,
    /// 2:1 squeezed pupil and long horizontal streaks.
    Anamorphic,
}
impl Default for LensProfile {
    fn default() -> Self {
        Self::Spherical
    }
}

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Lens Flare", category_color = "#E0B040")]
#[derive(Reflectable)]
#[serde(default)]
pub struct LensFlareProps {
    #[property(category = "Lens Flare")]
    pub enabled: bool,
    #[property(category = "Lens Flare")]
    pub quality: LensQuality,
    #[property(category = "Lens Flare")]
    pub profile: LensProfile,
    #[property(min = 0.0, max = 16.0, step = 0.01, category = "Lens Flare")]
    pub intensity: f32,
    /// Scene-linear luminance where the lens starts responding. Zero with a
    /// zero knee makes the response linear in scene radiance.
    #[property(min = 0.0, max = 1000.0, step = 0.1, category = "Lens Flare")]
    pub threshold: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Lens Flare")]
    pub soft_knee: f32,
    #[property(min = 0.0, max = 8.0, step = 1.0, category = "Lens Flare")]
    pub ghost_count: u32,
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Lens Flare")]
    pub ghost_intensity: f32,
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Lens Flare")]
    pub halo_intensity: f32,
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Lens Flare")]
    pub glare_intensity: f32,
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Lens Flare")]
    pub streak_intensity: f32,
    #[property(min = 0.0, max = 0.05, step = 0.001, category = "Lens Flare")]
    pub dispersion: f32,
    /// Diffraction spikes from the iris blades (the camera lens sets their count).
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Lens Flare")]
    pub starburst_intensity: f32,
    /// Spike length as a fraction of the image width.
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Lens Flare")]
    pub starburst_length: f32,
    /// Anti-reflection coating tint of ghosts (0 = neutral grey).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Lens Flare")]
    pub coating_strength: f32,
    /// Brighter, chromatically fringed ghost edges (0 = flat disks).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Lens Flare")]
    pub ghost_rim: f32,
    /// Front-element dirt lit by scattered and off-frame light.
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Lens Flare")]
    pub dirt_intensity: f32,
    /// Scene lights act as lens sources, so flares persist and fade
    /// physically as a light leaves the frame (shadow-map visibility).
    #[property(category = "Lens Flare")]
    pub light_sources: bool,
    #[property(min = 0.0, max = 16.0, step = 0.01, category = "Lens Flare")]
    pub light_intensity: f32,
    /// How far past the frame edge (fraction of the half-frame) light still
    /// reaches the front element.
    #[property(min = 0.0, max = 4.0, step = 0.01, category = "Lens Flare")]
    pub field_margin: f32,
    /// Seconds for the response to settle after occlusion, entering the
    /// frame or enabling: effects fade instead of popping (0 = instant).
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Lens Flare")]
    pub response_time: f32,
}

impl Default for LensFlareProps {
    fn default() -> Self {
        let value = LensFlareSettings::default();
        Self {
            enabled: value.enabled,
            quality: if value.quality == 1 {
                LensQuality::High
            } else {
                LensQuality::Economical
            },
            profile: if value.profile == 1 {
                LensProfile::Anamorphic
            } else {
                LensProfile::Spherical
            },
            ghost_count: value.ghost_count,
            intensity: value.intensity,
            threshold: value.threshold,
            soft_knee: value.soft_knee,
            ghost_intensity: value.ghost_intensity,
            halo_intensity: value.halo_intensity,
            glare_intensity: value.glare_intensity,
            streak_intensity: value.streak_intensity,
            dispersion: value.dispersion,
            starburst_intensity: value.starburst_intensity,
            starburst_length: value.starburst_length,
            coating_strength: value.coating_strength,
            ghost_rim: value.ghost_rim,
            dirt_intensity: value.dirt_intensity,
            light_sources: value.light_sources,
            light_intensity: value.light_intensity,
            field_margin: value.field_margin,
            response_time: value.response_time,
        }
    }
}

impl LensFlareProps {
    pub fn to_settings(&self) -> LensFlareSettings {
        LensFlareSettings {
            enabled: self.enabled,
            quality: match self.quality {
                LensQuality::Economical => 0,
                LensQuality::High => 1,
            },
            profile: match self.profile {
                LensProfile::Spherical => 0,
                LensProfile::Anamorphic => 1,
            },
            ghost_count: self.ghost_count.min(8),
            intensity: self.intensity,
            threshold: self.threshold,
            soft_knee: self.soft_knee,
            ghost_intensity: self.ghost_intensity,
            halo_intensity: self.halo_intensity,
            glare_intensity: self.glare_intensity,
            streak_intensity: self.streak_intensity,
            dispersion: self.dispersion,
            starburst_intensity: self.starburst_intensity,
            starburst_length: self.starburst_length,
            coating_strength: self.coating_strength,
            ghost_rim: self.ghost_rim,
            dirt_intensity: self.dirt_intensity,
            light_sources: self.light_sources,
            light_intensity: self.light_intensity,
            field_margin: self.field_margin,
            response_time: self.response_time,
        }
    }
}
