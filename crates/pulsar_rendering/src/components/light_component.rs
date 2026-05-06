//! Light component for scene lighting

use engine_class_derive::EngineClass;
use serde::{Deserialize, Serialize};

/// Light component for illuminating the scene
///
/// This component demonstrates:
/// - Enum properties (light type)
/// - Color properties (RGBA)
/// - Float properties with ranges
/// - Boolean properties for toggles
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct LightComponent {
    /// Type of light source
    #[property]
    pub light_type: LightType,

    /// Light intensity in lumens
    #[property(min = 0.0, max = 10000.0, step = 10.0)]
    pub intensity: f32,

    /// Light color (RGBA)
    #[property]
    pub color: [f32; 4],

    /// Maximum range of the light (for point and spot lights)
    #[property(min = 0.0, max = 1000.0, step = 1.0)]
    pub range: f32,

    /// Inner cone angle in degrees (for spot lights)
    #[property(min = 0.0, max = 90.0, step = 1.0)]
    pub inner_cone_angle: f32,

    /// Outer cone angle in degrees (for spot lights)
    #[property(min = 0.0, max = 90.0, step = 1.0)]
    pub outer_cone_angle: f32,

    /// Whether this light casts shadows
    #[property]
    pub cast_shadows: bool,

    /// Shadow map resolution (power of 2)
    #[property(min = 256.0, max = 4096.0, step = 256.0)]
    pub shadow_resolution: f32,

    /// Shadow bias to prevent shadow acne
    #[property(min = 0.0, max = 1.0, step = 0.001)]
    pub shadow_bias: f32,
}

/// Type of light source
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LightType {
    /// Directional light (like the sun) - infinite distance, parallel rays
    Directional,

    /// Point light - emits in all directions from a point
    Point,

    /// Spot light - emits in a cone from a point
    Spot,

    /// Area light - emits from a rectangular area
    Area,
}

impl Default for LightType {
    fn default() -> Self {
        LightType::Point
    }
}
