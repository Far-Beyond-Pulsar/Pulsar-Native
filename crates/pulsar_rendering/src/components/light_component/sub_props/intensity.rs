use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::IntensityUnits;

#[engine_class(clone, debug, serialize, deserialize)]
#[category("Intensity", category_color = "#F59E0B")]
pub struct IntensityLightProps {
    #[property(min = 0.0, max = 200000.0, step = 10.0, category = "Intensity")]
    pub intensity: f32,
    #[property(category = "Intensity")]
    pub intensity_units: IntensityUnits,
    #[property(min = -10.0, max = 10.0, step = 0.1, category = "Intensity")]
    pub exposure_compensation: f32,
    #[property(category = "Intensity")]
    pub inverse_squared_falloff: bool,
    #[property(min = 0.0, max = 16.0, step = 0.1, category = "Intensity")]
    pub indirect_intensity: f32,
    #[property(min = 0.0, max = 100000.0, step = 10.0, category = "Intensity")]
    pub max_draw_distance: f32,
    #[property(min = 0.0, max = 10000.0, step = 10.0, category = "Intensity")]
    pub max_distance_fade_range: f32,
}

impl Default for IntensityLightProps {
    fn default() -> Self {
        Self {
            intensity: 1000.0,
            intensity_units: IntensityUnits::Lumens,
            exposure_compensation: 0.0,
            inverse_squared_falloff: true,
            indirect_intensity: 1.0,
            max_draw_distance: 0.0,
            max_distance_fade_range: 0.0,
        }
    }
}

impl IntensityLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("intensity").and_then(|v| v.as_f64()) {
            self.intensity = v as f32;
        }
        if let Some(ix) = obj.get("intensity_units").and_then(|v| v.as_u64()) {
            self.intensity_units = match ix {
                0 => IntensityUnits::Unitless,
                1 => IntensityUnits::Lumens,
                2 => IntensityUnits::Candelas,
                3 => IntensityUnits::Lux,
                4 => IntensityUnits::Nits,
                _ => self.intensity_units,
            };
        }
        if let Some(v) = obj.get("exposure_compensation").and_then(|v| v.as_f64()) {
            self.exposure_compensation = v as f32;
        }
        if let Some(v) = obj.get("inverse_squared_falloff").and_then(|v| v.as_bool()) {
            self.inverse_squared_falloff = v;
        }
        if let Some(v) = obj.get("indirect_intensity").and_then(|v| v.as_f64()) {
            self.indirect_intensity = v as f32;
        }
        if let Some(v) = obj.get("max_draw_distance").and_then(|v| v.as_f64()) {
            self.max_draw_distance = v as f32;
        }
        if let Some(v) = obj.get("max_distance_fade_range").and_then(|v| v.as_f64()) {
            self.max_distance_fade_range = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("intensity".to_string(), Value::from(self.intensity));
        out.insert(
            "intensity_units".to_string(),
            Value::from(self.intensity_units as u64),
        );
        out.insert(
            "exposure_compensation".to_string(),
            Value::from(self.exposure_compensation),
        );
        out.insert(
            "inverse_squared_falloff".to_string(),
            Value::from(self.inverse_squared_falloff),
        );
        out.insert(
            "indirect_intensity".to_string(),
            Value::from(self.indirect_intensity),
        );
        out.insert(
            "max_draw_distance".to_string(),
            Value::from(self.max_draw_distance),
        );
        out.insert(
            "max_distance_fade_range".to_string(),
            Value::from(self.max_distance_fade_range),
        );
    }
}
