use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::{CollisionChannel, CollisionResponse, CollisionPreset};

#[engine_class(clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
pub struct GeneralPhysicsProps {
    #[property(category = "General")]
    pub enabled: bool,
    #[property(category = "General")]
    pub collision_enabled: bool,
    #[property(category = "General")]
    pub generate_overlap_events: bool,
    #[property(category = "General")]
    pub simulation_generates_hits: bool,
    #[property(category = "General")]
    pub collision_on_rotation: bool,
}

impl Default for GeneralPhysicsProps {
    fn default() -> Self {
        Self {
            enabled: true,
            collision_enabled: true,
            generate_overlap_events: true,
            simulation_generates_hits: true,
            collision_on_rotation: false,
        }
    }
}

impl GeneralPhysicsProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("enabled").and_then(|v| v.as_bool()) {
            self.enabled = v;
        }
        if let Some(v) = obj.get("collision_enabled").and_then(|v| v.as_bool()) {
            self.collision_enabled = v;
        }
        if let Some(v) = obj.get("generate_overlap_events").and_then(|v| v.as_bool()) {
            self.generate_overlap_events = v;
        }
        if let Some(v) = obj.get("simulation_generates_hits").and_then(|v| v.as_bool()) {
            self.simulation_generates_hits = v;
        }
        if let Some(v) = obj.get("collision_on_rotation").and_then(|v| v.as_bool()) {
            self.collision_on_rotation = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("enabled".to_string(), Value::from(self.enabled));
        out.insert("collision_enabled".to_string(), Value::from(self.collision_enabled));
        out.insert(
            "generate_overlap_events".to_string(),
            Value::from(self.generate_overlap_events),
        );
        out.insert(
            "simulation_generates_hits".to_string(),
            Value::from(self.simulation_generates_hits),
        );
        out.insert(
            "collision_on_rotation".to_string(),
            Value::from(self.collision_on_rotation),
        );
    }
}
