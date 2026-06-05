use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(clone, debug, serialize, deserialize)]
#[category("Attenuation", category_color = "#6EC5FF")]
pub struct AttenuationLightProps {
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Attenuation")]
    pub range: f32,
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Attenuation")]
    pub falloff_start: f32,
    #[property(min = 0.1, max = 16.0, step = 0.1, category = "Attenuation")]
    pub attenuation_exponent: f32,
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Attenuation")]
    pub source_radius: f32,
    #[property(min = 0.0, max = 200.0, step = 0.1, category = "Attenuation")]
    pub source_length: f32,
    #[property(min = 0.0, max = 90.0, step = 1.0, category = "Attenuation")]
    pub inner_cone_angle: f32,
    #[property(min = 0.0, max = 90.0, step = 1.0, category = "Attenuation")]
    pub outer_cone_angle: f32,
}

impl Default for AttenuationLightProps {
    fn default() -> Self {
        Self {
            range: 1000.0,
            falloff_start: 0.0,
            attenuation_exponent: 2.0,
            source_radius: 0.0,
            source_length: 0.0,
            inner_cone_angle: 30.0,
            outer_cone_angle: 45.0,
        }
    }
}

impl AttenuationLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("range").and_then(|v| v.as_f64()) {
            self.range = v as f32;
        }
        if let Some(v) = obj.get("falloff_start").and_then(|v| v.as_f64()) {
            self.falloff_start = v as f32;
        }
        if let Some(v) = obj.get("attenuation_exponent").and_then(|v| v.as_f64()) {
            self.attenuation_exponent = v as f32;
        }
        if let Some(v) = obj.get("source_radius").and_then(|v| v.as_f64()) {
            self.source_radius = v as f32;
        }
        if let Some(v) = obj.get("source_length").and_then(|v| v.as_f64()) {
            self.source_length = v as f32;
        }
        if let Some(v) = obj.get("inner_cone_angle").and_then(|v| v.as_f64()) {
            self.inner_cone_angle = v as f32;
        }
        if let Some(v) = obj.get("outer_cone_angle").and_then(|v| v.as_f64()) {
            self.outer_cone_angle = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("range".to_string(), Value::from(self.range));
        out.insert("falloff_start".to_string(), Value::from(self.falloff_start));
        out.insert(
            "attenuation_exponent".to_string(),
            Value::from(self.attenuation_exponent),
        );
        out.insert("source_radius".to_string(), Value::from(self.source_radius));
        out.insert("source_length".to_string(), Value::from(self.source_length));
        out.insert(
            "inner_cone_angle".to_string(),
            Value::from(self.inner_cone_angle),
        );
        out.insert(
            "outer_cone_angle".to_string(),
            Value::from(self.outer_cone_angle),
        );
    }
}
