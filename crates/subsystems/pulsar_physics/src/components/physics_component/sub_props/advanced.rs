use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::InterpolationMethod;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Advanced", category_color = "#9CA3AF", default_collapsed = true)]
pub struct AdvancedPhysicsProps {
    #[property(category = "Advanced")]
    pub enable_transform_interpolation: bool,
    #[property(category = "Advanced")]
    pub sync_to_physics: bool,
    #[property(category = "Advanced")]
    pub interpolation_method: InterpolationMethod,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Advanced")]
    pub min_translation_for_interpolation: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Advanced")]
    pub min_rotation_for_interpolation: f32,
    #[property(category = "Advanced")]
    pub override_linear_velocity: bool,
    #[property(category = "Advanced")]
    pub override_angular_velocity: bool,
}

impl Default for AdvancedPhysicsProps {
    fn default() -> Self {
        Self {
            enable_transform_interpolation: false,
            sync_to_physics: false,
            interpolation_method: InterpolationMethod::None,
            min_translation_for_interpolation: 0.01,
            min_rotation_for_interpolation: 1.0,
            override_linear_velocity: false,
            override_angular_velocity: false,
        }
    }
}

impl AdvancedPhysicsProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj
            .get("enable_transform_interpolation")
            .and_then(|v| v.as_bool())
        {
            self.enable_transform_interpolation = v;
        }
        if let Some(v) = obj.get("sync_to_physics").and_then(|v| v.as_bool()) {
            self.sync_to_physics = v;
        }
        if let Some(ix) = obj.get("interpolation_method").and_then(|v| v.as_u64()) {
            self.interpolation_method = match ix {
                0 => InterpolationMethod::None,
                1 => InterpolationMethod::Lerp,
                2 => InterpolationMethod::Slerp,
                _ => self.interpolation_method,
            };
        }
        if let Some(v) = obj
            .get("min_translation_for_interpolation")
            .and_then(|v| v.as_f64())
        {
            self.min_translation_for_interpolation = v as f32;
        }
        if let Some(v) = obj
            .get("min_rotation_for_interpolation")
            .and_then(|v| v.as_f64())
        {
            self.min_rotation_for_interpolation = v as f32;
        }
        if let Some(v) = obj
            .get("override_linear_velocity")
            .and_then(|v| v.as_bool())
        {
            self.override_linear_velocity = v;
        }
        if let Some(v) = obj
            .get("override_angular_velocity")
            .and_then(|v| v.as_bool())
        {
            self.override_angular_velocity = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "enable_transform_interpolation".to_string(),
            Value::from(self.enable_transform_interpolation),
        );
        out.insert(
            "sync_to_physics".to_string(),
            Value::from(self.sync_to_physics),
        );
        out.insert(
            "interpolation_method".to_string(),
            Value::from(self.interpolation_method as u64),
        );
        out.insert(
            "min_translation_for_interpolation".to_string(),
            Value::from(self.min_translation_for_interpolation),
        );
        out.insert(
            "min_rotation_for_interpolation".to_string(),
            Value::from(self.min_rotation_for_interpolation),
        );
        out.insert(
            "override_linear_velocity".to_string(),
            Value::from(self.override_linear_velocity),
        );
        out.insert(
            "override_angular_velocity".to_string(),
            Value::from(self.override_angular_velocity),
        );
    }
}
