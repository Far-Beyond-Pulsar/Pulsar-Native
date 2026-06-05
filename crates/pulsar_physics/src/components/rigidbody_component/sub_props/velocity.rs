use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Velocity", category_color = "#3B82F6")]
pub struct VelocityRigidbodyProps {
    #[property(category = "Velocity")]
    pub linear_velocity: [f32; 3],
    #[property(category = "Velocity")]
    pub angular_velocity: [f32; 3],
    #[property(category = "Velocity")]
    pub auto_compute_linear_velocity: bool,
    #[property(category = "Velocity")]
    pub auto_compute_angular_velocity: bool,
    #[property(category = "Velocity")]
    pub compute_velocity_from_displacement: bool,
}

impl Default for VelocityRigidbodyProps {
    fn default() -> Self {
        Self {
            linear_velocity: [0.0, 0.0, 0.0],
            angular_velocity: [0.0, 0.0, 0.0],
            auto_compute_linear_velocity: false,
            auto_compute_angular_velocity: false,
            compute_velocity_from_displacement: false,
        }
    }
}

impl VelocityRigidbodyProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(arr) = obj.get("linear_velocity").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.linear_velocity = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(arr) = obj.get("angular_velocity").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.angular_velocity = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(v) = obj.get("auto_compute_linear_velocity").and_then(|v| v.as_bool()) {
            self.auto_compute_linear_velocity = v;
        }
        if let Some(v) = obj.get("auto_compute_angular_velocity").and_then(|v| v.as_bool()) {
            self.auto_compute_angular_velocity = v;
        }
        if let Some(v) =
            obj.get("compute_velocity_from_displacement").and_then(|v| v.as_bool())
        {
            self.compute_velocity_from_displacement = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "linear_velocity".to_string(),
            serde_json::json!([
                self.linear_velocity[0],
                self.linear_velocity[1],
                self.linear_velocity[2]
            ]),
        );
        out.insert(
            "angular_velocity".to_string(),
            serde_json::json!([
                self.angular_velocity[0],
                self.angular_velocity[1],
                self.angular_velocity[2]
            ]),
        );
        out.insert(
            "auto_compute_linear_velocity".to_string(),
            Value::from(self.auto_compute_linear_velocity),
        );
        out.insert(
            "auto_compute_angular_velocity".to_string(),
            Value::from(self.auto_compute_angular_velocity),
        );
        out.insert(
            "compute_velocity_from_displacement".to_string(),
            Value::from(self.compute_velocity_from_displacement),
        );
    }
}
