use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use crate::components::physics_component::MotionType;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
pub struct GeneralRigidbodyProps {
    #[property(category = "General")]
    pub enabled: bool,
    #[property(category = "General")]
    pub mass: f32,
    #[property(min = 0.01, max = 100.0, step = 0.01, category = "General")]
    pub mass_scale: f32,
    #[property(category = "General")]
    pub density: f32,
    #[property(category = "General")]
    pub motion_type: MotionType,
    #[property(category = "General")]
    pub override_mass: bool,
}

impl Default for GeneralRigidbodyProps {
    fn default() -> Self {
        Self {
            enabled: true,
            mass: 1.0,
            mass_scale: 1.0,
            density: 1.0,
            motion_type: MotionType::Dynamic,
            override_mass: false,
        }
    }
}

impl GeneralRigidbodyProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("enabled").and_then(|v| v.as_bool()) {
            self.enabled = v;
        }
        if let Some(v) = obj.get("mass").and_then(|v| v.as_f64()) {
            self.mass = v as f32;
        }
        if let Some(v) = obj.get("mass_scale").and_then(|v| v.as_f64()) {
            self.mass_scale = v as f32;
        }
        if let Some(v) = obj.get("density").and_then(|v| v.as_f64()) {
            self.density = v as f32;
        }
        if let Some(ix) = obj.get("motion_type").and_then(|v| v.as_u64()) {
            self.motion_type = match ix {
                0 => MotionType::Dynamic,
                1 => MotionType::KinematicStatic,
                2 => MotionType::Static,
                _ => self.motion_type,
            };
        }
        if let Some(v) = obj.get("override_mass").and_then(|v| v.as_bool()) {
            self.override_mass = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("enabled".to_string(), Value::from(self.enabled));
        out.insert("mass".to_string(), Value::from(self.mass));
        out.insert("mass_scale".to_string(), Value::from(self.mass_scale));
        out.insert("density".to_string(), Value::from(self.density));
        out.insert(
            "motion_type".to_string(),
            Value::from(self.motion_type as u64),
        );
        out.insert("override_mass".to_string(), Value::from(self.override_mass));
    }
}
