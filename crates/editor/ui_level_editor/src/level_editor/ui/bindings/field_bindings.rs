//! Field binding system for type-safe, bidirectional data binding between UI and scene data
//!
//! This module provides a trait-based system for declaratively mapping UI input fields
//! to scene data fields with automatic bidirectional synchronization and undo/redo support.

use crate::level_editor::scene_database::{ObjectId, SceneDatabase, SceneObjectData};
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
    getter: Option<Arc<dyn Fn(&SceneObjectData) -> f32 + Send + Sync>>,
    setter: Option<Arc<dyn Fn(&mut SceneObjectData, f32) + Send + Sync>>,
    getter_db: Option<Arc<dyn Fn(&ObjectId, &SceneDatabase) -> Option<f32> + Send + Sync>>,
    setter_db: Option<Arc<dyn Fn(&ObjectId, f32, &SceneDatabase) -> bool + Send + Sync>>,
}

impl F32FieldBinding {
    pub fn new<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&SceneObjectData) -> f32 + Send + Sync + 'static,
        S: Fn(&mut SceneObjectData, f32) + Send + Sync + 'static,
    {
        Self {
            getter: Some(Arc::new(getter)),
            setter: Some(Arc::new(setter)),
            getter_db: None,
            setter_db: None,
        }
    }

    pub fn new_with_db<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&ObjectId, &SceneDatabase) -> Option<f32> + Send + Sync + 'static,
        S: Fn(&ObjectId, f32, &SceneDatabase) -> bool + Send + Sync + 'static,
    {
        Self {
            getter: None,
            setter: None,
            getter_db: Some(Arc::new(getter)),
            setter_db: Some(Arc::new(setter)),
        }
    }
}

impl FieldBinding for F32FieldBinding {
    type Value = f32;

    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<f32> {
        if let Some(getter_db) = &self.getter_db {
            return getter_db(object_id, db);
        }

        let getter = self.getter.as_ref()?;
        db.get_object(object_id).map(|obj| getter(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: f32, db: &SceneDatabase) -> bool {
        if let Some(setter_db) = &self.setter_db {
            return setter_db(object_id, value, db);
        }

        if let Some(mut obj) = db.get_object(object_id) {
            if let Some(setter) = &self.setter {
                setter(&mut obj, value);
                return db.update_object(obj); // Automatically records to undo/redo
            }
        }
        false
    }

    fn to_string(&self, value: &f32) -> String {
        format!("{:.3}", value)
    }

    fn from_string(&self, s: &str) -> Result<f32, String> {
        s.trim()
            .parse()
            .map_err(|_| format!("Invalid number: {}", s))
    }
}

// ============================================================================
// String Field Binding
// ============================================================================

/// Binding for a String field
pub struct StringFieldBinding {
    getter: Option<Arc<dyn Fn(&SceneObjectData) -> String + Send + Sync>>,
    setter: Option<Arc<dyn Fn(&mut SceneObjectData, String) + Send + Sync>>,
    getter_db: Option<Arc<dyn Fn(&ObjectId, &SceneDatabase) -> Option<String> + Send + Sync>>,
    setter_db: Option<Arc<dyn Fn(&ObjectId, String, &SceneDatabase) -> bool + Send + Sync>>,
}

impl StringFieldBinding {
    pub fn new<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&SceneObjectData) -> String + Send + Sync + 'static,
        S: Fn(&mut SceneObjectData, String) + Send + Sync + 'static,
    {
        Self {
            getter: Some(Arc::new(getter)),
            setter: Some(Arc::new(setter)),
            getter_db: None,
            setter_db: None,
        }
    }

    /// Same shape as `F32FieldBinding::new_with_db` -- see that method's doc.
    pub fn new_with_db<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&ObjectId, &SceneDatabase) -> Option<String> + Send + Sync + 'static,
        S: Fn(&ObjectId, String, &SceneDatabase) -> bool + Send + Sync + 'static,
    {
        Self {
            getter: None,
            setter: None,
            getter_db: Some(Arc::new(getter)),
            setter_db: Some(Arc::new(setter)),
        }
    }
}

impl FieldBinding for StringFieldBinding {
    type Value = String;

    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<String> {
        if let Some(getter_db) = &self.getter_db {
            return getter_db(object_id, db);
        }
        let getter = self.getter.as_ref()?;
        db.get_object(object_id).map(|obj| getter(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: String, db: &SceneDatabase) -> bool {
        if let Some(setter_db) = &self.setter_db {
            return setter_db(object_id, value, db);
        }
        if let Some(mut obj) = db.get_object(object_id) {
            if let Some(setter) = &self.setter {
                setter(&mut obj, value);
                return db.update_object(obj);
            }
        }
        false
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
    getter: Option<Arc<dyn Fn(&SceneObjectData) -> bool + Send + Sync>>,
    setter: Option<Arc<dyn Fn(&mut SceneObjectData, bool) + Send + Sync>>,
    getter_db: Option<Arc<dyn Fn(&ObjectId, &SceneDatabase) -> Option<bool> + Send + Sync>>,
    setter_db: Option<Arc<dyn Fn(&ObjectId, bool, &SceneDatabase) -> bool + Send + Sync>>,
}

impl BoolFieldBinding {
    pub fn new<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&SceneObjectData) -> bool + Send + Sync + 'static,
        S: Fn(&mut SceneObjectData, bool) + Send + Sync + 'static,
    {
        Self {
            getter: Some(Arc::new(getter)),
            setter: Some(Arc::new(setter)),
            getter_db: None,
            setter_db: None,
        }
    }

    /// Same shape as `F32FieldBinding::new_with_db` -- see that method's doc.
    pub fn new_with_db<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&ObjectId, &SceneDatabase) -> Option<bool> + Send + Sync + 'static,
        S: Fn(&ObjectId, bool, &SceneDatabase) -> bool + Send + Sync + 'static,
    {
        Self {
            getter: None,
            setter: None,
            getter_db: Some(Arc::new(getter)),
            setter_db: Some(Arc::new(setter)),
        }
    }
}

impl FieldBinding for BoolFieldBinding {
    type Value = bool;

    fn get(&self, object_id: &ObjectId, db: &SceneDatabase) -> Option<bool> {
        if let Some(getter_db) = &self.getter_db {
            return getter_db(object_id, db);
        }
        let getter = self.getter.as_ref()?;
        db.get_object(object_id).map(|obj| getter(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: bool, db: &SceneDatabase) -> bool {
        if let Some(setter_db) = &self.setter_db {
            return setter_db(object_id, value, db);
        }
        if let Some(mut obj) = db.get_object(object_id) {
            if let Some(setter) = &self.setter {
                setter(&mut obj, value);
                return db.update_object(obj);
            }
        }
        false
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};

    #[test]
    fn test_f32_binding() {
        let db = SceneDatabase::new();

        // Add test object
        let object_id = "test_object".to_string();
        db.add_object(
            SceneObjectData {
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
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            None,
        );

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
        assert_eq!(binding.to_string(&3.256), "3.256");

        // Test from_string
        assert_eq!(binding.from_string("2.5"), Ok(2.5));
        assert!(binding.from_string("invalid").is_err());
    }

    #[test]
    fn test_string_binding() {
        let binding = StringFieldBinding::new(|obj| obj.name.clone(), |obj, val| obj.name = val);

        assert_eq!(binding.to_string(&"Test".to_string()), "Test");
        assert_eq!(binding.from_string("Hello"), Ok("Hello".to_string()));
    }

    #[test]
    fn test_bool_binding() {
        let binding = BoolFieldBinding::new(|obj| obj.visible, |obj, val| obj.visible = val);

        assert_eq!(binding.to_string(&true), "true");
        assert_eq!(binding.to_string(&false), "false");

        assert_eq!(binding.from_string("true"), Ok(true));
        assert_eq!(binding.from_string("false"), Ok(false));
        assert_eq!(binding.from_string("1"), Ok(true));
        assert_eq!(binding.from_string("0"), Ok(false));
        assert!(binding.from_string("invalid").is_err());
    }
}
