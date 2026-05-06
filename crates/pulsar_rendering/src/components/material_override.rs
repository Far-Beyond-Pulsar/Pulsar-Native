//! Material override component for customizing object appearance

use engine_class_derive::EngineClass;
use serde::{Deserialize, Serialize};

/// Material override component for per-object material customization
///
/// Allows overriding material properties on a per-instance basis without
/// creating new material assets.
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct MaterialOverrideComponent {
    /// Base color tint
    #[property]
    pub base_color: [f32; 4],

    /// Metallic factor (0 = dielectric, 1 = metal)
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub metallic: f32,

    /// Roughness factor (0 = smooth/glossy, 1 = rough/matte)
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub roughness: f32,

    /// Emissive color (RGB, HDR values allowed)
    #[property]
    pub emissive_color: [f32; 3],

    /// Emissive intensity multiplier
    #[property(min = 0.0, max = 100.0, step = 0.1)]
    pub emissive_intensity: f32,

    /// Alpha/opacity (0 = fully transparent, 1 = fully opaque)
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub alpha: f32,

    /// UV scale for tiling textures
    #[property(min = 0.01, max = 100.0, step = 0.1)]
    pub uv_scale_x: f32,

    #[property(min = 0.01, max = 100.0, step = 0.1)]
    pub uv_scale_y: f32,

    /// UV offset for scrolling textures
    #[property(min = -10.0, max = 10.0, step = 0.01)]
    pub uv_offset_x: f32,

    #[property(min = -10.0, max = 10.0, step = 0.01)]
    pub uv_offset_y: f32,
}
