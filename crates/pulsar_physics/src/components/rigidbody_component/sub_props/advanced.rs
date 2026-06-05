use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use crate::components::physics_component::InterpolationMethod;

pub struct AdvancedRigidbodyProps {
    #[property(category = "Advanced")]
    pub enable_transform_interpolation: bool,
    #[property(category = "Advanced")]
    pub interpolation_method: InterpolationMethod,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Advanced")]
    pub min_translation_for_interpolation: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Advanced")]
    pub min_rotation_for_interpolation: f32,
    #[property(category = "Advanced")]
    pub enable_sync_to_physics: bool,
    #[property(category = "Advanced")]
    pub enable_sleeping: bool,
    #[property(min = -100.0, max = 100.0, step = 0.01, category = "Advanced")]
    pub sleep_threshold: f32,
    #[property(category = "Advanced")]
    pub wake_on_collision: bool,
    #[property(category = "Advanced")]
    pub disable_collision: bool,
    #[property(category = "Advanced")]
    pub enable_gravity: bool,
}

impl Default for AdvancedRigidbodyProps {
    fn default() -> Self {
        Self {
            enable_transform_interpolation: false,
            interpolation_method: InterpolationMethod::None,
            min_translation_for_interpolation: 0.01,
            min_rotation_for_interpolation: 1.0,
            enable_sync_to_physics: false,
            enable_sleeping: true,
            sleep_threshold: 0.0,
            wake_on_collision: true,
            disable_collision: false,
            enable_gravity: true,
        }
    }
}

impl AdvancedRigidbodyProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj
            .get("enable_transform_interpolation")
            .and_then(|v| v.as_bool())
        {
            self.enable_transform_interpolation = v;
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
        if let Some(v) = obj.get("enable_sync_to_physics").and_then(|v| v.as_bool()) {
            self.enable_sync_to_physics = v;
        }
        if let Some(v) = obj.get("enable_sleeping").and_then(|v| v.as_bool()) {
            self.enable_sleeping = v;
        }
        if let Some(v) = obj.get("sleep_threshold").and_then(|v| v.as_f64()) {
            self.sleep_threshold = v as f32;
        }
        if let Some(v) = obj.get("wake_on_collision").and_then(|v| v.as_bool()) {
            self.wake_on_collision = v;
        }
        if let Some(v) = obj.get("disable_collision").and_then(|v| v.as_bool()) {
            self.disable_collision = v;
        }
        if let Some(v) = obj.get("enable_gravity").and_then(|v| v.as_bool()) {
            self.enable_gravity = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "enable_transform_interpolation".to_string(),
            Value::from(self.enable_transform_interpolation),
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
            "enable_sync_to_physics".to_string(),
            Value::from(self.enable_sync_to_physics),
        );
        out.insert(
            "enable_sleeping".to_string(),
            Value::from(self.enable_sleeping),
        );
        out.insert(
            "sleep_threshold".to_string(),
            Value::from(self.sleep_threshold),
        );
        out.insert(
            "wake_on_collision".to_string(),
            Value::from(self.wake_on_collision),
        );
        out.insert(
            "disable_collision".to_string(),
            Value::from(self.disable_collision),
        );
        out.insert(
            "enable_gravity".to_string(),
            Value::from(self.enable_gravity),
        );
    }
}
