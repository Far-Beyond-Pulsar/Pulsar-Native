use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

pub struct ConstraintsRigidbodyProps {
    #[property(category = "Constraints")]
    pub lock_linear_x: bool,
    #[property(category = "Constraints")]
    pub lock_linear_y: bool,
    #[property(category = "Constraints")]
    pub lock_linear_z: bool,
    #[property(category = "Constraints")]
    pub lock_angular_x: bool,
    #[property(category = "Constraints")]
    pub lock_angular_y: bool,
    #[property(category = "Constraints")]
    pub lock_angular_z: bool,
    #[property(category = "Constraints")]
    pub auto_update_constraints: bool,
    #[property(category = "Constraints")]
    pub enable_locked_motions_override: bool,
}

impl Default for ConstraintsRigidbodyProps {
    fn default() -> Self {
        Self {
            lock_linear_x: false,
            lock_linear_y: false,
            lock_linear_z: false,
            lock_angular_x: false,
            lock_angular_y: false,
            lock_angular_z: false,
            auto_update_constraints: true,
            enable_locked_motions_override: false,
        }
    }
}

impl ConstraintsRigidbodyProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("lock_linear_x").and_then(|v| v.as_bool()) {
            self.lock_linear_x = v;
        }
        if let Some(v) = obj.get("lock_linear_y").and_then(|v| v.as_bool()) {
            self.lock_linear_y = v;
        }
        if let Some(v) = obj.get("lock_linear_z").and_then(|v| v.as_bool()) {
            self.lock_linear_z = v;
        }
        if let Some(v) = obj.get("lock_angular_x").and_then(|v| v.as_bool()) {
            self.lock_angular_x = v;
        }
        if let Some(v) = obj.get("lock_angular_y").and_then(|v| v.as_bool()) {
            self.lock_angular_y = v;
        }
        if let Some(v) = obj.get("lock_angular_z").and_then(|v| v.as_bool()) {
            self.lock_angular_z = v;
        }
        if let Some(v) = obj.get("auto_update_constraints").and_then(|v| v.as_bool()) {
            self.auto_update_constraints = v;
        }
        if let Some(v) = obj
            .get("enable_locked_motions_override")
            .and_then(|v| v.as_bool())
        {
            self.enable_locked_motions_override = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("lock_linear_x".to_string(), Value::from(self.lock_linear_x));
        out.insert("lock_linear_y".to_string(), Value::from(self.lock_linear_y));
        out.insert("lock_linear_z".to_string(), Value::from(self.lock_linear_z));
        out.insert(
            "lock_angular_x".to_string(),
            Value::from(self.lock_angular_x),
        );
        out.insert(
            "lock_angular_y".to_string(),
            Value::from(self.lock_angular_y),
        );
        out.insert(
            "lock_angular_z".to_string(),
            Value::from(self.lock_angular_z),
        );
        out.insert(
            "auto_update_constraints".to_string(),
            Value::from(self.auto_update_constraints),
        );
        out.insert(
            "enable_locked_motions_override".to_string(),
            Value::from(self.enable_locked_motions_override),
        );
    }
}
