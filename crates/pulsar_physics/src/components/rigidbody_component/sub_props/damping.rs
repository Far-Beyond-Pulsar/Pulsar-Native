use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(clone, debug, serialize, deserialize)]
#[category("Damping", category_color = "#8B5CF6")]
pub struct DampingRigidbodyProps {
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Damping")]
    pub linear_damping: f32,
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Damping")]
    pub angular_damping: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Damping")]
    pub default_linear_damping: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Damping")]
    pub default_angular_damping: f32,
    #[property(category = "Damping")]
    pub disable_animation_damping: bool,
    #[property(category = "Damping")]
    pub disable_pose_animation_damping: bool,
}

impl Default for DampingRigidbodyProps {
    fn default() -> Self {
        Self {
            linear_damping: 0.0,
            angular_damping: 0.0,
            default_linear_damping: 0.0,
            default_angular_damping: 0.0,
            disable_animation_damping: false,
            disable_pose_animation_damping: false,
        }
    }
}

impl DampingRigidbodyProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("linear_damping").and_then(|v| v.as_f64()) {
            self.linear_damping = v as f32;
        }
        if let Some(v) = obj.get("angular_damping").and_then(|v| v.as_f64()) {
            self.angular_damping = v as f32;
        }
        if let Some(v) = obj.get("default_linear_damping").and_then(|v| v.as_f64()) {
            self.default_linear_damping = v as f32;
        }
        if let Some(v) = obj.get("default_angular_damping").and_then(|v| v.as_f64()) {
            self.default_angular_damping = v as f32;
        }
        if let Some(v) = obj.get("disable_animation_damping").and_then(|v| v.as_bool()) {
            self.disable_animation_damping = v;
        }
        if let Some(v) = obj.get("disable_pose_animation_damping").and_then(|v| v.as_bool()) {
            self.disable_pose_animation_damping = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("linear_damping".to_string(), Value::from(self.linear_damping));
        out.insert("angular_damping".to_string(), Value::from(self.angular_damping));
        out.insert(
            "default_linear_damping".to_string(),
            Value::from(self.default_linear_damping),
        );
        out.insert(
            "default_angular_damping".to_string(),
            Value::from(self.default_angular_damping),
        );
        out.insert(
            "disable_animation_damping".to_string(),
            Value::from(self.disable_animation_damping),
        );
        out.insert(
            "disable_pose_animation_damping".to_string(),
            Value::from(self.disable_pose_animation_damping),
        );
    }
}
