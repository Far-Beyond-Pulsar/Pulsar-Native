use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Material", category_color = "#F97316", default_collapsed = false)]
pub struct MaterialTerrainProps {
    #[property(category = "Material")]
    pub base_color: [f32; 4],
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Material")]
    pub roughness: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Material")]
    pub metallic: f32,
    #[property(category = "Material")]
    pub material_override: String,
}

impl Default for MaterialTerrainProps {
    fn default() -> Self {
        Self {
            base_color: [0.5, 0.5, 0.4, 1.0],
            roughness: 0.8,
            metallic: 0.0,
            material_override: String::new(),
        }
    }
}

impl MaterialTerrainProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(arr) = obj.get("base_color").and_then(|v| v.as_array()) {
            for (i, v) in arr.iter().enumerate().take(4) {
                if let Some(n) = v.as_f64() {
                    self.base_color[i] = n as f32;
                }
            }
        }
        if let Some(v) = obj.get("roughness").and_then(|v| v.as_f64()).map(|v| v as f32) {
            self.roughness = v;
        }
        if let Some(v) = obj.get("metallic").and_then(|v| v.as_f64()).map(|v| v as f32) {
            self.metallic = v;
        }
        if let Some(v) = obj.get("material_override").and_then(|v| v.as_str()) {
            self.material_override = v.to_string();
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("base_color".to_string(), Value::from(Vec::from(self.base_color)));
        out.insert("roughness".to_string(), Value::from(self.roughness));
        out.insert("metallic".to_string(), Value::from(self.metallic));
        out.insert("material_override".to_string(), Value::from(self.material_override.clone()));
    }
}
