use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Transform", category_color = "#A78BFA")]
pub struct TransformTerrainProps {
    #[property(category = "Transform")]
    pub position: [f32; 3],
    #[property(category = "Transform")]
    pub rotation: [f32; 3],
    #[property(min = 1.0, max = 100000.0, step = 100.0, category = "Transform")]
    pub world_size: f32,
}

impl Default for TransformTerrainProps {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            world_size: 10000.0,
        }
    }
}

impl TransformTerrainProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(arr) = obj.get("position").and_then(|v| v.as_array()) {
            for (i, v) in arr.iter().enumerate().take(3) {
                if let Some(n) = v.as_f64() {
                    self.position[i] = n as f32;
                }
            }
        }
        if let Some(arr) = obj.get("rotation").and_then(|v| v.as_array()) {
            for (i, v) in arr.iter().enumerate().take(3) {
                if let Some(n) = v.as_f64() {
                    self.rotation[i] = n as f32;
                }
            }
        }
        if let Some(v) = obj.get("world_size").and_then(|v| v.as_f64()).map(|v| v as f32) {
            self.world_size = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "position".to_string(),
            Value::from(self.position.to_vec()),
        );
        out.insert(
            "rotation".to_string(),
            Value::from(self.rotation.to_vec()),
        );
        out.insert("world_size".to_string(), Value::from(self.world_size));
    }
}
