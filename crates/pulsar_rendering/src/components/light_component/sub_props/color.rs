use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(clone, debug, serialize, deserialize)]
#[category("Color", category_color = "#FF8AAE")]
pub struct ColorLightProps {
    #[property(category = "Color")]
    pub color: [f32; 4],
    #[property(category = "Color")]
    pub use_temperature: bool,
    #[property(min = 1000.0, max = 20000.0, step = 50.0, category = "Color")]
    pub temperature_kelvin: f32,
    #[property(min = -1.0, max = 1.0, step = 0.01, category = "Color")]
    pub temperature_tint: f32,
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Color")]
    pub color_saturation: f32,
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Color")]
    pub color_contrast: f32,
    #[property(category = "Color")]
    pub use_physical_light_color: bool,
}

impl Default for ColorLightProps {
    fn default() -> Self {
        Self {
            color: [1.0, 1.0, 1.0, 1.0],
            use_temperature: false,
            temperature_kelvin: 6500.0,
            temperature_tint: 0.0,
            color_saturation: 1.0,
            color_contrast: 1.0,
            use_physical_light_color: true,
        }
    }
}

impl ColorLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(arr) = obj.get("color").and_then(|v| v.as_array())
            && arr.len() >= 4
        {
            self.color = [
                arr[0].as_f64().unwrap_or(1.0) as f32,
                arr[1].as_f64().unwrap_or(1.0) as f32,
                arr[2].as_f64().unwrap_or(1.0) as f32,
                arr[3].as_f64().unwrap_or(1.0) as f32,
            ];
        }
        if let Some(v) = obj.get("use_temperature").and_then(|v| v.as_bool()) {
            self.use_temperature = v;
        }
        if let Some(v) = obj.get("temperature_kelvin").and_then(|v| v.as_f64()) {
            self.temperature_kelvin = v as f32;
        }
        if let Some(v) = obj.get("temperature_tint").and_then(|v| v.as_f64()) {
            self.temperature_tint = v as f32;
        }
        if let Some(v) = obj.get("color_saturation").and_then(|v| v.as_f64()) {
            self.color_saturation = v as f32;
        }
        if let Some(v) = obj.get("color_contrast").and_then(|v| v.as_f64()) {
            self.color_contrast = v as f32;
        }
        if let Some(v) = obj.get("use_physical_light_color").and_then(|v| v.as_bool()) {
            self.use_physical_light_color = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "color".to_string(),
            serde_json::json!([self.color[0], self.color[1], self.color[2], self.color[3]]),
        );
        out.insert(
            "use_temperature".to_string(),
            Value::from(self.use_temperature),
        );
        out.insert(
            "temperature_kelvin".to_string(),
            Value::from(self.temperature_kelvin),
        );
        out.insert(
            "temperature_tint".to_string(),
            Value::from(self.temperature_tint),
        );
        out.insert(
            "color_saturation".to_string(),
            Value::from(self.color_saturation),
        );
        out.insert(
            "color_contrast".to_string(),
            Value::from(self.color_contrast),
        );
        out.insert(
            "use_physical_light_color".to_string(),
            Value::from(self.use_physical_light_color),
        );
    }
}
