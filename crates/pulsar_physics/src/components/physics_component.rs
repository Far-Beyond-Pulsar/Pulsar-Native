//! Physics component with collider support

use engine_class_derive::EngineClass;
use pulsar_reflection::*;
use serde::{Deserialize, Serialize};

/// Physics component that defines mass, friction, and colliders
///
/// This component demonstrates:
/// - Numeric properties with constraints (min/max/step)
/// - Boolean properties
/// - Vec<T> properties for dynamic arrays
/// - Nested component support (ColliderDescriptor)
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
#[category("Physics")]
pub struct PhysicsComponent {
    /// Mass of the object in kilograms
    #[property(min = 0.0, max = 1000.0)]
    pub mass: f32,

    /// Friction coefficient (0 = frictionless, 1 = maximum friction)
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub friction: f32,

    /// Restitution/bounciness (0 = no bounce, 1 = perfect bounce)
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    pub restitution: f32,

    /// Whether this object is kinematic (moved by script, not physics)
    #[property]
    pub kinematic: bool,

    /// Colliders attached to this physics object
    /// Users can add/remove colliders with +/- buttons in the UI
    #[property]
    pub colliders: Vec<ColliderDescriptor>,
}

/// Describes a single collider shape
///
/// This is a nested component that will be rendered inline when editing
/// the PhysicsComponent's colliders array.
#[derive(EngineClass, Default, Clone, Debug, Serialize, Deserialize)]
pub struct ColliderDescriptor {
    /// Type of collider shape
    #[property]
    pub shape: ColliderShape,

    /// Offset from the object's center
    #[property]
    pub offset: [f32; 3],

    /// Size/dimensions of the collider
    #[property(min = 0.01)]
    pub size: [f32; 3],

    /// Whether this collider is a trigger (doesn't block, just detects)
    #[property]
    pub is_trigger: bool,
}

/// Collider shape types
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ColliderShape {
    /// Axis-aligned box collider
    Box,

    /// Sphere collider
    Sphere,

    /// Capsule collider (cylinder with hemispherical ends)
    Capsule,

    /// Mesh collider (uses object's mesh geometry)
    Mesh,
}

impl Default for ColliderShape {
    fn default() -> Self {
        ColliderShape::Box
    }
}

static COLLIDER_DESCRIPTOR_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<ColliderDescriptor>(),
    type_name: "pulsar_physics::ColliderDescriptor",
    size: std::mem::size_of::<ColliderDescriptor>(),
    align: std::mem::align_of::<ColliderDescriptor>(),
    structure: TypeStructure::Primitive,
};

impl Reflectable for ColliderDescriptor {
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized,
    {
        &COLLIDER_DESCRIPTOR_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        serializer.serialize_registered(self as &dyn std::any::Any)
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
                expected: "ColliderDescriptor",
                found,
            })
    }

    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(self.clone())
    }
}

fn serialize_collider_descriptor_json(
    value: &dyn std::any::Any,
) -> ReflectResult<serde_json::Value> {
    let collider =
        value
            .downcast_ref::<ColliderDescriptor>()
            .ok_or_else(|| ReflectError::TypeMismatch {
                expected: "ColliderDescriptor",
                found: format!("{:?}", value.type_id()),
            })?;

    serde_json::to_value(collider).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_collider_descriptor_json(
    value: serde_json::Value,
) -> ReflectResult<Box<dyn std::any::Any>> {
    let collider: ColliderDescriptor = serde_json::from_value(value)
        .map_err(|e| ReflectError::DeserializationFailed(e.to_string()))?;
    Ok(Box::new(collider))
}

pulsar_reflection::inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &COLLIDER_DESCRIPTOR_TYPE_INFO,
        serialize_json: serialize_collider_descriptor_json,
        deserialize_json: deserialize_collider_descriptor_json,
    }
}

static COLLIDER_SHAPE_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: std::any::TypeId::of::<ColliderShape>(),
    type_name: "pulsar_physics::ColliderShape",
    size: std::mem::size_of::<ColliderShape>(),
    align: std::mem::align_of::<ColliderShape>(),
    structure: TypeStructure::Primitive,
};

impl Reflectable for ColliderShape {
    fn type_info() -> &'static RuntimeTypeInfo
    where
        Self: Sized,
    {
        &COLLIDER_SHAPE_TYPE_INFO
    }

    fn serialize(&self, serializer: &mut dyn TypeSerializer) -> ReflectResult<()> {
        serializer.serialize_registered(self as &dyn std::any::Any)
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
                expected: "ColliderShape",
                found,
            })
    }

    fn clone_any(&self) -> Box<dyn std::any::Any> {
        Box::new(*self)
    }
}

fn serialize_collider_shape_json(value: &dyn std::any::Any) -> ReflectResult<serde_json::Value> {
    let shape =
        value
            .downcast_ref::<ColliderShape>()
            .ok_or_else(|| ReflectError::TypeMismatch {
                expected: "ColliderShape",
                found: format!("{:?}", value.type_id()),
            })?;

    serde_json::to_value(shape).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_collider_shape_json(
    value: serde_json::Value,
) -> ReflectResult<Box<dyn std::any::Any>> {
    let shape: ColliderShape = serde_json::from_value(value)
        .map_err(|e| ReflectError::DeserializationFailed(e.to_string()))?;
    Ok(Box::new(shape))
}

pulsar_reflection::inventory::submit! {
    RuntimeTypeRegistration {
        type_info: &COLLIDER_SHAPE_TYPE_INFO,
        serialize_json: serialize_collider_shape_json,
        deserialize_json: deserialize_collider_shape_json,
    }
}
