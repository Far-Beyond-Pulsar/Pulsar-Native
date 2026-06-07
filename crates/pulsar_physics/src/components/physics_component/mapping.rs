use serde_json::Value;
use std::collections::HashMap;

use super::PhysicsComponent;

impl PhysicsComponent {
    pub fn from_component_data(data: &Value) -> Self {
        let mut physics = Self::default();
        if let Some(obj) = data.as_object() {
            physics.general.apply_from_component_data(obj);
            physics.collision.apply_from_component_data(obj);
            physics.material.apply_from_component_data(obj);
            physics.simulation.apply_from_component_data(obj);
            physics.advanced.apply_from_component_data(obj);
        }
        physics
    }

    pub fn to_scene_props(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        self.general.apply_to_scene_props(&mut out);
        self.collision.apply_to_scene_props(&mut out);
        self.material.apply_to_scene_props(&mut out);
        self.simulation.apply_to_scene_props(&mut out);
        self.advanced.apply_to_scene_props(&mut out);
        out
    }
}
