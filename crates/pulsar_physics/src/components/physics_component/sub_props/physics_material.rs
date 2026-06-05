use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(clone, debug, serialize, deserialize)]
#[category("Physics Material", category_color = "#4ECDC4")]
pub struct MaterialPhysicsProps {
    #[property(category = "Physics Material")]
    pub physics_material: String,
    #[property(category = "Physics Material")]
    pub override_physics_material: bool,
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Physics Material")]
    pub friction: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Physics Material")]
    pub restitution: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Physics Material")]
    pub combined_friction: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Physics Material")]
    pub combined_restitution: f32,
}

impl Default for MaterialPhysicsProps {
    fn default() -> Self {
        Self {
            physics_material: String::new(),
            override_physics_material: false,
            friction: 0.4,
            restitution: 0.2,
            combined_friction: 0.4,
            combined_restitution: 0.2,
        }
    }
}

impl MaterialPhysicsProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("physics_material").and_then(|v| v.as_str()) {
            self.physics_material = v.to_string();
        }
        if let Some(v) = obj.get("override_physics_material").and_then(|v| v.as_bool()) {
            self.override_physics_material = v;
        }
        if let Some(v) = obj.get("friction").and_then(|v| v.as_f64()) {
            self.friction = v as f32;
        }
        if let Some(v) = obj.get("restitution").and_then(|v| v.as_f64()) {
            self.restitution = v as f32;
        }
        if let Some(v) = obj.get("combined_friction").and_then(|v| v.as_f64()) {
            self.combined_friction = v as f32;
        }
        if let Some(v) = obj.get("combined_restitution").and_then(|v| v.as_f64()) {
            self.combined_restitution = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "physics_material".to_string(),
            Value::from(self.physics_material.clone()),
        );
        out.insert(
            "override_physics_material".to_string(),
            Value::from(self.override_physics_material),
        );
        out.insert("friction".to_string(), Value::from(self.friction));
        out.insert("restitution".to_string(), Value::from(self.restitution));
        out.insert("combined_friction".to_string(), Value::from(self.combined_friction));
        out.insert(
            "combined_restitution".to_string(),
            Value::from(self.combined_restitution),
        );
    }
}
