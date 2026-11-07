use engine_class_derive::engine_class;
use serde_json::Value;

use super::super::IntensityUnits;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Intensity", category_color = "#F59E0B")]
// Fields added later load from levels saved before them.
#[serde(default)]
pub struct IntensityLightProps {
    #[property(min = 0.0, max = 200000.0, step = 10.0, category = "Intensity")]
    #[gpu]
    pub intensity: f32,
    #[property(category = "Intensity")]
    #[gpu(as = u32, with = intensity_units_to_gpu)]
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

pub fn intensity_units_to_gpu(units: IntensityUnits) -> u32 {
    units as u32
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
}
