//! Level of Detail (LOD) component for performance optimization

use engine_class_derive::EngineClass;
use serde::{Deserialize, Serialize};

/// LOD component for managing mesh detail based on distance
///
/// This component demonstrates Vec<T> properties where each LOD level
/// can be added/removed dynamically in the UI.
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Rendering")]
pub struct LODComponent {
    /// LOD levels with their distance thresholds
    /// Users can add/remove LOD levels with +/- buttons
    #[property]
    pub lod_levels: Vec<LODLevel>,

    /// Whether to animate transitions between LOD levels
    #[property]
    pub smooth_transitions: bool,

    /// Transition duration in seconds
    #[property(min = 0.0, max = 2.0, step = 0.1)]
    pub transition_duration: f32,

    /// Bias to prefer higher or lower LOD (negative = higher quality, positive = lower quality)
    #[property(min = -2.0, max = 2.0, step = 0.1)]
    pub lod_bias: f32,
}

/// Single LOD level descriptor
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
pub struct LODLevel {
    /// Distance from camera where this LOD becomes active
    #[property(min = 0.0, max = 10000.0, step = 1.0)]
    pub distance_threshold: f32,

    /// Screen space coverage percentage (0-100) where this LOD becomes active
    #[property(min = 0.0, max = 100.0, step = 0.1)]
    pub screen_coverage: f32,

    /// Mesh asset path for this LOD level
    /// In a full implementation, this would be a proper asset reference
    #[property]
    pub mesh_path: String,
}
