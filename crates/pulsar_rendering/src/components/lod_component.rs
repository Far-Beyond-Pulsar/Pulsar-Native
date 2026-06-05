//! Level of Detail (LOD) component for performance optimization

use engine_class_derive::engine_class;
use pulsar_reflection::{
    ReflectError, ReflectResult, pulsar_type,
};
use serde::{Deserialize, Serialize};

/// LOD component for managing mesh detail based on distance
///
/// This component demonstrates Vec<T> properties where each LOD level
/// can be added/removed dynamically in the UI.
#[engine_class(category = "Rendering", default, clone, debug, serialize, deserialize)]
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
#[engine_class(default, clone, debug, serialize, deserialize)]
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

fn serialize_lod_level_json(value: &LODLevel) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_lod_level_json(value: serde_json::Value) -> ReflectResult<LODLevel> {
    serde_json::from_value(value).map_err(|e| ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_type(
    primitive,
    serialize_json_with = serialize_lod_level_json,
    deserialize_json_with = deserialize_lod_level_json
)]
pub type RegisteredLodLevel = LODLevel;
