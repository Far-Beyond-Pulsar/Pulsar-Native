//! Physics component with collider support

use engine_class_derive::engine_class;
use pulsar_reflection::*;
use serde::{Deserialize, Serialize};

/// Physics component that defines mass, friction, and colliders
///
/// This component demonstrates:
/// - Numeric properties with constraints (min/max/step)
/// - Boolean properties
/// - Vec<T> properties for dynamic arrays
/// - Nested component support (ColliderDescriptor)
#[engine_class(category = "Physics", default, clone, debug, serialize, deserialize)]
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
#[engine_class(default, clone, debug, serialize, deserialize)]
#[category("Shape", category_color = "#58A6FF", default_collapsed = true)]
#[category("Transform", default_collapsed = true)]
#[category("Collision")]
pub struct ColliderDescriptor {
    /// Type of collider shape
    #[property(category = "Shape", category_color = "#58A6FF")]
    pub shape: ColliderShape,

    /// Offset from the object's center
    #[property(category = "Transform")]
    pub offset: [f32; 3],

    /// Size/dimensions of the collider
    #[property(min = 0.01, category = "Transform")]
    pub size: [f32; 3],

    /// Whether this collider is a trigger (doesn't block, just detects)
    #[property(category = "Collision")]
    pub is_trigger: bool,
}

fn serialize_collider_descriptor_json(
    value: &ColliderDescriptor,
) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_collider_descriptor_json(
    value: serde_json::Value,
) -> ReflectResult<ColliderDescriptor> {
    serde_json::from_value(value).map_err(|e| ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_type(
    primitive,
    serialize_json_with = serialize_collider_descriptor_json,
    deserialize_json_with = deserialize_collider_descriptor_json
)]
pub type RegisteredColliderDescriptor = ColliderDescriptor;

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

#[pulsar_type(
    primitive,
    serialize_json_with = serialize_collider_shape_json,
    deserialize_json_with = deserialize_collider_shape_json,
    editor = render_collider_shape_editor
)]
pub type RegisteredColliderShape = ColliderShape;

fn serialize_collider_shape_json(value: &ColliderShape) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_collider_shape_json(
    value: serde_json::Value,
) -> Result<ColliderShape, ReflectError> {
    let shape = serde_json::from_value(value)
        .map_err(|e| ReflectError::DeserializationFailed(e.to_string()))?;
    Ok(shape)
}

fn render_collider_shape_editor(
    args: &pulsar_reflection::PropertyEditorArgs<'_>,
    cx: &gpui::App,
) -> gpui::AnyElement {
    use gpui::{Corner, prelude::*, *};
    use ui::{ActiveTheme, color_picker::ColorPicker, h_flex};

    div().child("Hello world").into_any_element()
}
