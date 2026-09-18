//! Field binding system for type-safe, bidirectional data binding between UI and scene data
//!
//! This module provides a trait-based system for declaratively mapping UI input fields
//! to scene data fields with automatic bidirectional synchronization and undo/redo support.

use crate::level_editor::scene_edit::{ObjectId, SceneObjectData};
use engine_backend::scene::SharedScene;
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
    fn get(&self, object_id: &ObjectId, db: &SharedScene) -> Option<Self::Value>;

    /// Set a new value in the scene database (automatically records to undo/redo history)
    fn set(&self, object_id: &ObjectId, value: Self::Value, db: &SharedScene) -> bool;

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
    getter_db: Option<Arc<dyn Fn(&ObjectId, &SharedScene) -> Option<f32> + Send + Sync>>,
    setter_db: Option<Arc<dyn Fn(&ObjectId, f32, &SharedScene) -> bool + Send + Sync>>,
}

impl F32FieldBinding {
    pub fn new_with_db<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&ObjectId, &SharedScene) -> Option<f32> + Send + Sync + 'static,
        S: Fn(&ObjectId, f32, &SharedScene) -> bool + Send + Sync + 'static,
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

    fn get(&self, object_id: &ObjectId, db: &SharedScene) -> Option<f32> {
        if let Some(getter_db) = &self.getter_db {
            return getter_db(object_id, db);
        }

        let getter = self.getter.as_ref()?;
        let world = db.read();
        crate::level_editor::scene_edit::objects::get_object(&world.world, object_id)
            .map(|obj| getter(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: f32, db: &SharedScene) -> bool {
        if let Some(setter_db) = &self.setter_db {
            return setter_db(object_id, value, db);
        }

        let mut world = db.write();
        if let Some(mut obj) =
            crate::level_editor::scene_edit::objects::get_object(&world.world, object_id)
        {
            if let Some(setter) = &self.setter {
                setter(&mut obj, value);
                return crate::level_editor::scene_edit::objects::update_object(&mut world.world, obj);
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
    getter_db: Option<Arc<dyn Fn(&ObjectId, &SharedScene) -> Option<String> + Send + Sync>>,
    setter_db: Option<Arc<dyn Fn(&ObjectId, String, &SharedScene) -> bool + Send + Sync>>,
}

impl StringFieldBinding {
    /// Same shape as `F32FieldBinding::new_with_db` -- see that method's doc.
    pub fn new_with_db<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&ObjectId, &SharedScene) -> Option<String> + Send + Sync + 'static,
        S: Fn(&ObjectId, String, &SharedScene) -> bool + Send + Sync + 'static,
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

    fn get(&self, object_id: &ObjectId, db: &SharedScene) -> Option<String> {
        if let Some(getter_db) = &self.getter_db {
            return getter_db(object_id, db);
        }
        let getter = self.getter.as_ref()?;
        let world = db.read();
        crate::level_editor::scene_edit::objects::get_object(&world.world, object_id)
            .map(|obj| getter(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: String, db: &SharedScene) -> bool {
        if let Some(setter_db) = &self.setter_db {
            return setter_db(object_id, value, db);
        }
        let mut world = db.write();
        if let Some(mut obj) =
            crate::level_editor::scene_edit::objects::get_object(&world.world, object_id)
        {
            if let Some(setter) = &self.setter {
                setter(&mut obj, value);
                return crate::level_editor::scene_edit::objects::update_object(&mut world.world, obj);
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
    getter_db: Option<Arc<dyn Fn(&ObjectId, &SharedScene) -> Option<bool> + Send + Sync>>,
    setter_db: Option<Arc<dyn Fn(&ObjectId, bool, &SharedScene) -> bool + Send + Sync>>,
}

impl BoolFieldBinding {
    /// Same shape as `F32FieldBinding::new_with_db` -- see that method's doc.
    pub fn new_with_db<G, S>(getter: G, setter: S) -> Self
    where
        G: Fn(&ObjectId, &SharedScene) -> Option<bool> + Send + Sync + 'static,
        S: Fn(&ObjectId, bool, &SharedScene) -> bool + Send + Sync + 'static,
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

    fn get(&self, object_id: &ObjectId, db: &SharedScene) -> Option<bool> {
        if let Some(getter_db) = &self.getter_db {
            return getter_db(object_id, db);
        }
        let getter = self.getter.as_ref()?;
        let world = db.read();
        crate::level_editor::scene_edit::objects::get_object(&world.world, object_id)
            .map(|obj| getter(&obj))
    }

    fn set(&self, object_id: &ObjectId, value: bool, db: &SharedScene) -> bool {
        if let Some(setter_db) = &self.setter_db {
            return setter_db(object_id, value, db);
        }
        let mut world = db.write();
        if let Some(mut obj) =
            crate::level_editor::scene_edit::objects::get_object(&world.world, object_id)
        {
            if let Some(setter) = &self.setter {
                setter(&mut obj, value);
                return crate::level_editor::scene_edit::objects::update_object(&mut world.world, obj);
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
