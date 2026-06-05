use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

pub struct ForcesRigidbodyProps {
    #[property(category = "Forces")]
    pub gravity_enabled: bool,
    #[property(min = -10.0, max = 10.0, step = 0.1, category = "Forces")]
    pub gravity_scale: f32,
    #[property(category = "Forces")]
    pub custom_gravity: [f32; 3],
    #[property(category = "Forces")]
    pub apply_force: [f32; 3],
    #[property(category = "Forces")]
    pub apply_force_position: [f32; 3],
    #[property(category = "Forces")]
    pub apply_impulse: [f32; 3],
    #[property(category = "Forces")]
    pub apply_impulse_position: [f32; 3],
    #[property(category = "Forces")]
    pub apply_torque: [f32; 3],
    #[property(category = "Forces")]
    pub apply_angular_impulse: [f32; 3],
    #[property(category = "Forces")]
    pub disable_all_forces: bool,
}

impl Default for ForcesRigidbodyProps {
    fn default() -> Self {
        Self {
            gravity_enabled: true,
            gravity_scale: 1.0,
            custom_gravity: [0.0, -981.0, 0.0],
            apply_force: [0.0, 0.0, 0.0],
            apply_force_position: [0.0, 0.0, 0.0],
            apply_impulse: [0.0, 0.0, 0.0],
            apply_impulse_position: [0.0, 0.0, 0.0],
            apply_torque: [0.0, 0.0, 0.0],
            apply_angular_impulse: [0.0, 0.0, 0.0],
            disable_all_forces: false,
        }
    }
}

impl ForcesRigidbodyProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("gravity_enabled").and_then(|v| v.as_bool()) {
            self.gravity_enabled = v;
        }
        if let Some(v) = obj.get("gravity_scale").and_then(|v| v.as_f64()) {
            self.gravity_scale = v as f32;
        }
        if let Some(arr) = obj.get("custom_gravity").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.custom_gravity = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(-981.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(arr) = obj.get("apply_force").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.apply_force = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(arr) = obj.get("apply_force_position").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.apply_force_position = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(arr) = obj.get("apply_impulse").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.apply_impulse = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(arr) = obj.get("apply_impulse_position").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.apply_impulse_position = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(arr) = obj.get("apply_torque").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.apply_torque = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(arr) = obj.get("apply_angular_impulse").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.apply_angular_impulse = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(v) = obj.get("disable_all_forces").and_then(|v| v.as_bool()) {
            self.disable_all_forces = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "gravity_enabled".to_string(),
            Value::from(self.gravity_enabled),
        );
        out.insert("gravity_scale".to_string(), Value::from(self.gravity_scale));
        out.insert(
            "custom_gravity".to_string(),
            serde_json::json!([
                self.custom_gravity[0],
                self.custom_gravity[1],
                self.custom_gravity[2]
            ]),
        );
        out.insert(
            "apply_force".to_string(),
            serde_json::json!([
                self.apply_force[0],
                self.apply_force[1],
                self.apply_force[2]
            ]),
        );
        out.insert(
            "apply_force_position".to_string(),
            serde_json::json!([
                self.apply_force_position[0],
                self.apply_force_position[1],
                self.apply_force_position[2]
            ]),
        );
        out.insert(
            "apply_impulse".to_string(),
            serde_json::json!([
                self.apply_impulse[0],
                self.apply_impulse[1],
                self.apply_impulse[2]
            ]),
        );
        out.insert(
            "apply_impulse_position".to_string(),
            serde_json::json!([
                self.apply_impulse_position[0],
                self.apply_impulse_position[1],
                self.apply_impulse_position[2]
            ]),
        );
        out.insert(
            "apply_torque".to_string(),
            serde_json::json!([
                self.apply_torque[0],
                self.apply_torque[1],
                self.apply_torque[2]
            ]),
        );
        out.insert(
            "apply_angular_impulse".to_string(),
            serde_json::json!([
                self.apply_angular_impulse[0],
                self.apply_angular_impulse[1],
                self.apply_angular_impulse[2]
            ]),
        );
        out.insert(
            "disable_all_forces".to_string(),
            Value::from(self.disable_all_forces),
        );
    }
}
