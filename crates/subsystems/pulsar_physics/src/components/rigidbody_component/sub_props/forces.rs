use engine_class_derive::engine_class;
use serde_json::Value;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Forces", category_color = "#F59E0B")]
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

}
