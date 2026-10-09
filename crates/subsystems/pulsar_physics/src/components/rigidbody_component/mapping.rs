use serde_json::Value;

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
}
