//! World Settings Data - Stores actual world/scene configuration values
//!
//! This module provides the data model for world settings that can be replicated
//! across multiuser sessions.

use serde::{Deserialize, Serialize};
use gpui::Hsla;

/// World settings data that can be serialized and replicated
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorldSettingsData {
    // Environment
    pub sky_color: [f32; 4], // RGBA
    pub horizon_color: [f32; 4],
    pub ground_color: [f32; 4],
    pub sky_intensity: f32,
    pub enable_clouds: bool,
    pub skybox: String,

    // Global Illumination
    pub ambient_color: [f32; 4],
    pub ambient_intensity: f32,
    pub gi_mode: GIMode,
    pub bounce_count: u32,
    pub realtime_gi: bool,
    pub ambient_occlusion: bool,

    // Fog & Atmosphere
    pub enable_fog: bool,
    pub fog_mode: FogMode,
    pub fog_color: [f32; 4],
    pub fog_density: f32,
    pub fog_start: f32,
    pub fog_end: f32,

    // Physics
    pub gravity: [f32; 3],
    pub time_scale: f32,
    pub fixed_timestep: f32,
    pub enable_physics: bool,
    pub auto_simulation: bool,

    // Audio
    pub master_volume: f32,
    pub speed_of_sound: f32,
    pub doppler_factor: f32,
    pub reverb_preset: ReverbPreset,
    pub enable_spatial_audio: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GIMode {
    Disabled,
    Baked,
    Realtime,
}

impl std::fmt::Display for GIMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GIMode::Disabled => write!(f, "Disabled"),
            GIMode::Baked => write!(f, "Baked"),
            GIMode::Realtime => write!(f, "Realtime"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FogMode {
    Linear,
    Exponential,
    ExponentialSquared,
}

impl std::fmt::Display for FogMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FogMode::Linear => write!(f, "Linear"),
            FogMode::Exponential => write!(f, "Exponential"),
            FogMode::ExponentialSquared => write!(f, "Exponential²"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReverbPreset {
    None,
    SmallRoom,
    LargeRoom,
    Hall,
    Cave,
    Arena,
}

impl std::fmt::Display for ReverbPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReverbPreset::None => write!(f, "None"),
            ReverbPreset::SmallRoom => write!(f, "Small Room"),
            ReverbPreset::LargeRoom => write!(f, "Large Room"),
            ReverbPreset::Hall => write!(f, "Hall"),
            ReverbPreset::Cave => write!(f, "Cave"),
            ReverbPreset::Arena => write!(f, "Arena"),
        }
    }
}

impl Default for WorldSettingsData {
    fn default() -> Self {
        Self {
            // Environment
            sky_color: hsla_to_rgba(Hsla { h: 210.0, s: 0.6, l: 0.7, a: 1.0 }),
            horizon_color: hsla_to_rgba(Hsla { h: 30.0, s: 0.7, l: 0.8, a: 1.0 }),
            ground_color: hsla_to_rgba(Hsla { h: 30.0, s: 0.3, l: 0.3, a: 1.0 }),
            sky_intensity: 1.0,
            enable_clouds: true,
            skybox: "Default Sky".to_string(),

            // Global Illumination
            ambient_color: hsla_to_rgba(Hsla { h: 220.0, s: 0.2, l: 0.4, a: 1.0 }),
            ambient_intensity: 0.3,
            gi_mode: GIMode::Baked,
            bounce_count: 2,
            realtime_gi: false,
            ambient_occlusion: true,

            // Fog & Atmosphere
            enable_fog: true,
            fog_mode: FogMode::Exponential,
            fog_color: hsla_to_rgba(Hsla { h: 210.0, s: 0.3, l: 0.7, a: 1.0 }),
            fog_density: 0.02,
            fog_start: 10.0,
            fog_end: 500.0,

            // Physics
            gravity: [0.0, -9.81, 0.0],
            time_scale: 1.0,
            fixed_timestep: 0.02,
            enable_physics: true,
            auto_simulation: true,

            // Audio
            master_volume: 1.0,
            speed_of_sound: 343.0,
            doppler_factor: 1.0,
            reverb_preset: ReverbPreset::None,
            enable_spatial_audio: true,
        }
    }
}

impl WorldSettingsData {
    /// Apply world settings to the engine/renderer
    pub fn apply(&self) {
        // TODO: Wire up to actual renderer/physics/audio systems
        tracing::info!("Applying world settings: gravity={:?}, time_scale={}",
            self.gravity, self.time_scale);
    }

    /// Serialize to JSON for network transmission
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// Convert HSLA color to RGBA array for storage
fn hsla_to_rgba(hsla: Hsla) -> [f32; 4] {
    // For simplicity, store as HSLA values directly
    // TODO: Convert to actual RGBA if needed by renderer
    [hsla.h / 360.0, hsla.s, hsla.l, hsla.a]
}

/// Convert RGBA array back to HSLA color
pub fn rgba_to_hsla(rgba: [f32; 4]) -> Hsla {
    Hsla {
        h: rgba[0] * 360.0,
        s: rgba[1],
        l: rgba[2],
        a: rgba[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization() {
        let settings = WorldSettingsData::default();
        let json = settings.to_json().unwrap();
        let deserialized = WorldSettingsData::from_json(&json).unwrap();

        assert_eq!(settings.sky_intensity, deserialized.sky_intensity);
        assert_eq!(settings.enable_fog, deserialized.enable_fog);
    }

    #[test]
    fn test_enum_display() {
        assert_eq!(GIMode::Baked.to_string(), "Baked");
        assert_eq!(FogMode::Exponential.to_string(), "Exponential");
        assert_eq!(ReverbPreset::Hall.to_string(), "Hall");
    }
}
