use serde_json::Value;

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
}
