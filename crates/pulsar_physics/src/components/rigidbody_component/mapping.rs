use serde_json::Value;
use std::collections::HashMap;

use super::RigidbodyComponent;

impl RigidbodyComponent {
    pub fn from_component_data(data: &Value) -> Self {
        let mut rigidbody = Self::default();
        if let Some(obj) = data.as_object() {
            rigidbody.general.apply_from_component_data(obj);
            rigidbody.velocity.apply_from_component_data(obj);
            rigidbody.damping.apply_from_component_data(obj);
            rigidbody.forces.apply_from_component_data(obj);
            rigidbody.constraints.apply_from_component_data(obj);
            rigidbody.advanced.apply_from_component_data(obj);
        }
        rigidbody
    }

    pub fn to_scene_props(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        self.general.apply_to_scene_props(&mut out);
        self.velocity.apply_to_scene_props(&mut out);
        self.damping.apply_to_scene_props(&mut out);
        self.forces.apply_to_scene_props(&mut out);
        self.constraints.apply_to_scene_props(&mut out);
        self.advanced.apply_to_scene_props(&mut out);
        out
    }
}
