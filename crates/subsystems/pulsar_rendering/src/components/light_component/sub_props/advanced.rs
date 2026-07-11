use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Advanced", category_color = "#9CA3AF", default_collapsed = true)]
pub struct AdvancedLightProps {
    #[property(category = "Advanced")]
    pub affects_translucency: bool,
    #[property(category = "Advanced")]
    pub affects_reflections: bool,
    #[property(category = "Advanced")]
    pub affects_global_illumination: bool,
    #[property(min = 0.0, max = 8.0, step = 0.01, category = "Advanced")]
    pub specular_scale: f32,
    #[property(min = 0.0, max = 8.0, step = 0.01, category = "Advanced")]
    pub diffuse_scale: f32,
}

impl Default for AdvancedLightProps {
    fn default() -> Self {
        Self {
            affects_translucency: true,
            affects_reflections: true,
            affects_global_illumination: true,
            specular_scale: 1.0,
            diffuse_scale: 1.0,
        }
    }
}

impl AdvancedLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("affects_translucency").and_then(|v| v.as_bool()) {
            self.affects_translucency = v;
        }
        if let Some(v) = obj.get("affects_reflections").and_then(|v| v.as_bool()) {
            self.affects_reflections = v;
        }
        if let Some(v) = obj
            .get("affects_global_illumination")
            .and_then(|v| v.as_bool())
        {
            self.affects_global_illumination = v;
        }
        if let Some(v) = obj.get("specular_scale").and_then(|v| v.as_f64()) {
            self.specular_scale = v as f32;
        }
        if let Some(v) = obj.get("diffuse_scale").and_then(|v| v.as_f64()) {
            self.diffuse_scale = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "affects_translucency".to_string(),
            Value::from(self.affects_translucency),
        );
        out.insert(
            "affects_reflections".to_string(),
            Value::from(self.affects_reflections),
        );
        out.insert(
            "affects_global_illumination".to_string(),
            Value::from(self.affects_global_illumination),
        );
        out.insert(
            "specular_scale".to_string(),
            Value::from(self.specular_scale),
        );
        out.insert("diffuse_scale".to_string(), Value::from(self.diffuse_scale));
    }
}
