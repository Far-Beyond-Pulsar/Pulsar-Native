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
