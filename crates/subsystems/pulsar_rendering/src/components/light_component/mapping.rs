use serde_json::Value;
use std::collections::HashMap;

use super::LightComponent;

impl LightComponent {
    pub fn from_component_data(data: &Value) -> Self {
        let mut light = Self::default();
        if let Some(obj) = data.as_object() {
            light.general.apply_from_component_data(obj);
            light.intensity.apply_from_component_data(obj);
            light.color.apply_from_component_data(obj);
            light.attenuation.apply_from_component_data(obj);
            light.shadows.apply_from_component_data(obj);
            light.volumetrics.apply_from_component_data(obj);
            light.light_function.apply_from_component_data(obj);
            light.performance.apply_from_component_data(obj);
            light.advanced.apply_from_component_data(obj);
        }
        light
    }

    pub fn to_scene_props(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        self.general.apply_to_scene_props(&mut out);
        self.intensity.apply_to_scene_props(&mut out);
        self.color.apply_to_scene_props(&mut out);
        self.attenuation.apply_to_scene_props(&mut out);
        self.shadows.apply_to_scene_props(&mut out);
        self.volumetrics.apply_to_scene_props(&mut out);
        self.light_function.apply_to_scene_props(&mut out);
        self.performance.apply_to_scene_props(&mut out);
        self.advanced.apply_to_scene_props(&mut out);
        out
    }
}
