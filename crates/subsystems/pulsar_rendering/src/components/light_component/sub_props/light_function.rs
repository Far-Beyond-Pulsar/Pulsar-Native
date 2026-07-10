use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Light Function", category_color = "#22D3EE", default_collapsed = true)]
pub struct LightFunctionProps {
    #[property(category = "Light Function")]
    pub light_function_material: String,
    #[property(category = "Light Function")]
    pub light_function_scale: [f32; 3],
    #[property(min = 0.0, max = 100000.0, step = 10.0, category = "Light Function")]
    pub light_function_fade_distance: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Light Function")]
    pub light_function_disabled_brightness: f32,
}

impl Default for LightFunctionProps {
    fn default() -> Self {
        Self {
            light_function_material: String::new(),
            light_function_scale: [1.0, 1.0, 1.0],
            light_function_fade_distance: 0.0,
            light_function_disabled_brightness: 0.0,
        }
    }
}

impl LightFunctionProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("light_function_material").and_then(|v| v.as_str()) {
            self.light_function_material = v.to_string();
        }
        if let Some(arr) = obj.get("light_function_scale").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.light_function_scale = [
                arr[0].as_f64().unwrap_or(1.0) as f32,
                arr[1].as_f64().unwrap_or(1.0) as f32,
                arr[2].as_f64().unwrap_or(1.0) as f32,
            ];
        }
        if let Some(v) = obj
            .get("light_function_fade_distance")
            .and_then(|v| v.as_f64())
        {
            self.light_function_fade_distance = v as f32;
        }
        if let Some(v) = obj
            .get("light_function_disabled_brightness")
            .and_then(|v| v.as_f64())
        {
            self.light_function_disabled_brightness = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "light_function_material".to_string(),
            Value::from(self.light_function_material.clone()),
        );
        out.insert(
            "light_function_scale".to_string(),
            serde_json::json!([
                self.light_function_scale[0],
                self.light_function_scale[1],
                self.light_function_scale[2]
            ]),
        );
        out.insert(
            "light_function_fade_distance".to_string(),
            Value::from(self.light_function_fade_distance),
        );
        out.insert(
            "light_function_disabled_brightness".to_string(),
            Value::from(self.light_function_disabled_brightness),
        );
    }
}
