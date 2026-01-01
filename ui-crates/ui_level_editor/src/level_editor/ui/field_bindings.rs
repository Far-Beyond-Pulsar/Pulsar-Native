//! Field binding system for type-safe, bidirectional data binding between UI and scene data
//!
//! This module provides a trait-based system for declaratively mapping UI input fields
//! to scene data fields with automatic bidirectional synchronization and undo/redo support.

use crate::level_editor::scene_database::{SceneDatabase, SceneObjectData, ObjectId};
use std::sync::Arc;

/// Core trait for field bindings that connect UI inputs to scene data
///
/// Implementing this trait allows a field to:
/// - Read values from the scene database
/// - Write values to the scene database (with automatic undo/redo)
/// - Convert between data types and UI string representations
/// - Validate user input
pub trait FieldBinding: 'static + Send + Sync {
    /// The value type this binding manages (f32, [f32; 3], String, etc.)
    type Value: Clone + PartialEq + Send + 'static;

    /// Get the current value from the scene database for the given object
    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<Self::Value>;

    /// Set a new value in the scene database (automatically records to undo/redo history)
    fn set(&self, object_id: &ObjectId, value: Self::Value, db: &SceneDatabase) -> bool;

    /// Convert value to string for display in UI
    fn to_string(&self, value: &Self::Value) -> String;

    /// Parse string from UI back to value
    fn from_string(&self, s: &str) -> Result<Self::Value, String>;

    /// Optional: Validate value before setting (override for custom validation)
    fn validate(&self, _value: &Self::Value) -> Result<(), String> {
        Ok(())
    }
}

// ============================================================================
// F32 Field Binding
// ============================================================================

/// Binding for a single f32 field
pub struct F32FieldBinding {
    getter: Arc<dyn Fn(&SceneObjectData) -> f32 + Send + Sync>,
    setter: Arc<dyn Fn(&mut SceneObjectData, f32) + Send + Sync>,
}

impl F32FieldBinding {
    pub fn new<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&SceneObjectData) -> f32 + Send + Sync + 'static,
        S: Fn(&mut SceneObjectData, f32) + Send + Sync + 'static,
    {
        Self {
            getter: Arc::new(getter),
            setter: Arc::new(setter),
        }
    }
}

impl FieldBinding for F32FieldBinding {
    type Value = f32;

    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<f32> {
        db.get_object(object_id).map(|obj| (self.getter)(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: f32, db: &SceneDatabase) -> bool {
        if let Some(mut obj) = db.get_object(object_id) {
            (self.setter)(&mut obj, value);
            db.update_object(obj) // Automatically records to undo/redo
        } else {
            false
        }
    }

    fn to_string(&self, value: &f32) -> String {
        format!("{:.3}", value)
    }

    fn from_string(&self, s: &str) -> Result<f32, String> {
        s.trim().parse().map_err(|_| format!("Invalid number: {}", s))
    }
}

// ============================================================================
// Vec3 Field Binding ([f32; 3])
// ============================================================================

/// Binding for a [f32; 3] field (position, rotation, scale)
pub struct Vec3FieldBinding {
    getter: Arc<dyn Fn(&SceneObjectData) -> [f32; 3] + Send + Sync>,
    setter: Arc<dyn Fn(&mut SceneObjectData, [f32; 3]) + Send + Sync>,
}

impl Vec3FieldBinding {
    pub fn new<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&SceneObjectData) -> [f32; 3] + Send + Sync + 'static,
        S: Fn(&mut SceneObjectData, [f32; 3]) + Send + Sync + 'static,
    {
        Self {
            getter: Arc::new(getter),
            setter: Arc::new(setter),
        }
    }
}

impl FieldBinding for Vec3FieldBinding {
    type Value = [f32; 3];

    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<[f32; 3]> {
        db.get_object(object_id).map(|obj| (self.getter)(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: [f32; 3], db: &SceneDatabase) -> bool {
        if let Some(mut obj) = db.get_object(object_id) {
            (self.setter)(&mut obj, value);
            db.update_object(obj)
        } else {
            false
        }
    }

    fn to_string(&self, value: &[f32; 3]) -> String {
        format!("[{:.3}, {:.3}, {:.3}]", value[0], value[1], value[2])
    }

    fn from_string(&self, s: &str) -> Result<[f32; 3], String> {
        let s = s.trim();

        // Try parsing "[x, y, z]" format
        if s.starts_with('[') && s.ends_with(']') {
            let inner = &s[1..s.len()-1];
            let parts: Vec<&str> = inner.split(',').collect();

            if parts.len() == 3 {
                let x = parts[0].trim().parse::<f32>().map_err(|_| format!("Invalid X value"))?;
                let y = parts[1].trim().parse::<f32>().map_err(|_| format!("Invalid Y value"))?;
                let z = parts[2].trim().parse::<f32>().map_err(|_| format!("Invalid Z value"))?;
                return Ok([x, y, z]);
            }
        }

        Err(format!("Invalid Vec3 format. Expected [x, y, z]"))
    }
}

// ============================================================================
// String Field Binding
// ============================================================================

/// Binding for a String field
pub struct StringFieldBinding {
    getter: Arc<dyn Fn(&SceneObjectData) -> String + Send + Sync>,
    setter: Arc<dyn Fn(&mut SceneObjectData, String) + Send + Sync>,
}

impl StringFieldBinding {
    pub fn new<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&SceneObjectData) -> String + Send + Sync + 'static,
        S: Fn(&mut SceneObjectData, String) + Send + Sync + 'static,
    {
        Self {
            getter: Arc::new(getter),
            setter: Arc::new(setter),
        }
    }
}

impl FieldBinding for StringFieldBinding {
    type Value = String;

    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<String> {
        db.get_object(object_id).map(|obj| (self.getter)(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: String, db: &SceneDatabase) -> bool {
        if let Some(mut obj) = db.get_object(object_id) {
            (self.setter)(&mut obj, value);
            db.update_object(obj)
        } else {
            false
        }
    }

    fn to_string(&self, value: &String) -> String {
        value.clone()
    }

    fn from_string(&self, s: &str) -> Result<String, String> {
        Ok(s.to_string())
    }
}

// ============================================================================
// Bool Field Binding
// ============================================================================

/// Binding for a boolean field
pub struct BoolFieldBinding {
    getter: Arc<dyn Fn(&SceneObjectData) -> bool + Send + Sync>,
    setter: Arc<dyn Fn(&mut SceneObjectData, bool) + Send + Sync>,
}

impl BoolFieldBinding {
    pub fn new<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&SceneObjectData) -> bool + Send + Sync + 'static,
        S: Fn(&mut SceneObjectData, bool) + Send + Sync + 'static,
    {
        Self {
            getter: Arc::new(getter),
            setter: Arc::new(setter),
        }
    }
}

impl FieldBinding for BoolFieldBinding {
    type Value = bool;

    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<bool> {
        db.get_object(object_id).map(|obj| (self.getter)(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: bool, db: &SceneDatabase) -> bool {
        if let Some(mut obj) = db.get_object(object_id) {
            (self.setter)(&mut obj, value);
            db.update_object(obj)
        } else {
            false
        }
    }

    fn to_string(&self, value: &bool) -> String {
        value.to_string()
    }

    fn from_string(&self, s: &str) -> Result<bool, String> {
        match s.trim().to_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(format!("Invalid boolean: {}", s)),
        }
    }
}

// ============================================================================
// Declarative Macros for Easy Binding Creation
// ============================================================================

/// Create an F32 field binding with custom getter/setter closures
///
/// # Examples
///
/// ```rust
/// // Bind to transform.position[0]
/// let binding = bind_f32_field!(
///     get: |obj| obj.transform.position[0],
///     set: |obj, val| obj.transform.position[0] = val
/// );
///
/// // Bind to a conditional field
/// let binding = bind_f32_field!(
///     get: |obj| match &obj.object_type {
///         ObjectType::Light(LightType::Point { intensity, .. }) => *intensity,
///         _ => 1.0,
///     },
///     set: |obj, val| {
///         if let ObjectType::Light(LightType::Point { intensity, .. }) = &mut obj.object_type {
///             *intensity = val;
///         }
///     }
/// );
/// ```
#[macro_export]
macro_rules! bind_f32_field {
    (get: |$obj_get:ident| $get_expr:expr, set: |$obj_set:ident, $val:ident| $set_expr:expr) => {{
        $crate::level_editor::ui::field_bindings::F32FieldBinding::new(
            |$obj_get| $get_expr,
            |$obj_set, $val| $set_expr,
        )
    }};
}

/// Create a Vec3 field binding with custom getter/setter closures
#[macro_export]
macro_rules! bind_vec3_field {
    (get: |$obj_get:ident| $get_expr:expr, set: |$obj_set:ident, $val:ident| $set_expr:expr) => {{
        $crate::level_editor::ui::field_bindings::Vec3FieldBinding::new(
            |$obj_get| $get_expr,
            |$obj_set, $val| $set_expr,
        )
    }};
}

/// Create a String field binding with custom getter/setter closures
#[macro_export]
macro_rules! bind_string_field {
    (get: |$obj_get:ident| $get_expr:expr, set: |$obj_set:ident, $val:ident| $set_expr:expr) => {{
        $crate::level_editor::ui::field_bindings::StringFieldBinding::new(
            |$obj_get| $get_expr,
            |$obj_set, $val| $set_expr,
        )
    }};
}

/// Create a Bool field binding with custom getter/setter closures
#[macro_export]
macro_rules! bind_bool_field {
    (get: |$obj_get:ident| $get_expr:expr, set: |$obj_set:ident, $val:ident| $set_expr:expr) => {{
        $crate::level_editor::ui::field_bindings::BoolFieldBinding::new(
            |$obj_get| $get_expr,
            |$obj_set, $val| $set_expr,
        )
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level_editor::scene_database::{SceneObjectData, ObjectType, Transform};

    #[test]
    fn test_f32_binding() {
        let db = SceneDatabase::new();

        // Add test object
        let object_id = "test_object".to_string();
        db.add_object(SceneObjectData {
            id: object_id.clone(),
            name: "Test".to_string(),
            object_type: ObjectType::Empty,
            transform: Transform {
                position: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0],
                scale: [1.0, 1.0, 1.0],
            },
            parent: None,
            children: Vec::new(),
            visible: true,
            locked: false,
            components: Vec::new(),
        }, None);

        // Create binding for position.x
        let binding = F32FieldBinding::new(
            |obj| obj.transform.position[0],
            |obj, val| obj.transform.position[0] = val,
        );

        // Test get
        assert_eq!(binding.get(&object_id, &db), Some(1.0));

        // Test set
        assert!(binding.set(&object_id, 5.0, &db));
        assert_eq!(binding.get(&object_id, &db), Some(5.0));

        // Test to_string
        assert_eq!(binding.to_string(&3.14159), "3.142");

        // Test from_string
        assert_eq!(binding.from_string("2.5"), Ok(2.5));
        assert!(binding.from_string("invalid").is_err());
    }

    #[test]
    fn test_vec3_binding() {
        let binding = Vec3FieldBinding::new(
            |obj| obj.transform.position,
            |obj, val| obj.transform.position = val,
        );

        // Test to_string
        assert_eq!(binding.to_string(&[1.0, 2.5, 3.14]), "[1.000, 2.500, 3.140]");

        // Test from_string
        assert_eq!(binding.from_string("[1, 2, 3]"), Ok([1.0, 2.0, 3.0]));
        assert_eq!(binding.from_string("[1.5, 2.5, 3.5]"), Ok([1.5, 2.5, 3.5]));
        assert!(binding.from_string("invalid").is_err());
        assert!(binding.from_string("[1, 2]").is_err());
    }

    #[test]
    fn test_string_binding() {
        let binding = StringFieldBinding::new(
            |obj| obj.name.clone(),
            |obj, val| obj.name = val,
        );

        assert_eq!(binding.to_string(&"Test".to_string()), "Test");
        assert_eq!(binding.from_string("Hello"), Ok("Hello".to_string()));
    }

    #[test]
    fn test_bool_binding() {
        let binding = BoolFieldBinding::new(
            |obj| obj.visible,
            |obj, val| obj.visible = val,
        );

        assert_eq!(binding.to_string(&true), "true");
        assert_eq!(binding.to_string(&false), "false");

        assert_eq!(binding.from_string("true"), Ok(true));
        assert_eq!(binding.from_string("false"), Ok(false));
        assert_eq!(binding.from_string("1"), Ok(true));
        assert_eq!(binding.from_string("0"), Ok(false));
        assert!(binding.from_string("invalid").is_err());
    }
}
