//! Level of Detail (LOD) component for performance optimization

use engine_class_derive::EngineClass;
use pulsar_reflection::{
    ReflectError, ReflectResult, Reflectable, RuntimeTypeInfo, RuntimeTypeRegistration,
    TypeDeserializer, TypeSerializer, TypeStructure,
};
use serde::{Deserialize, Serialize};
use std::any::Any;

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

static LOD_LEVEL_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<LODLevel>(),
    type_name: "pulsar_rendering::LODLevel",
    size: std::mem::size_of::<LODLevel>(),
    align: std::mem::align_of::<LODLevel>(),
    structure: TypeStructure::Primitive,
};

impl Reflectable for LODLevel {
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized,
    {
        &LOD_LEVEL_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        serializer.serialize_registered(self as &dyn Any)
    }

    fn deserialize(deserializer: &mut dyn TypeDeserializer) -> ReflectResult<Self>
    where
        Self: Sized,
    {
        let boxed = deserializer.deserialize_registered(Self::type_info())?;
        let found = format!("{:?}", (&*boxed).type_id());
        boxed
            .downcast::<Self>()
            .map(|v| *v)
            .map_err(|_| ReflectError::TypeMismatch {
                expected: "LODLevel",
                found,
            })
    }

    fn clone_any(&self) -> Box<dyn Any> {
        Box::new(self.clone())
    }
}

fn serialize_lod_level_json(value: &dyn Any) -> ReflectResult<serde_json::Value> {
    let lod = value
        .downcast_ref::<LODLevel>()
        .ok_or_else(|| ReflectError::TypeMismatch {
            expected: "LODLevel",
            found: format!("{:?}", value.type_id()),
        })?;

    serde_json::to_value(lod).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_lod_level_json(value: serde_json::Value) -> ReflectResult<Box<dyn Any>> {
    let lod: LODLevel = serde_json::from_value(value)
        .map_err(|e| ReflectError::DeserializationFailed(e.to_string()))?;
    Ok(Box::new(lod))
}

pulsar_reflection::inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &LOD_LEVEL_TYPE_INFO,
        serialize_json: serialize_lod_level_json,
        deserialize_json: deserialize_lod_level_json,
    }
}
