use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Placement", category_color = "#6EC5FF")]
pub struct PlacementFoliageProps {
    /// Minimum blade/plant height in metres.
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Placement")]
    pub height_min: f32,
    /// Maximum blade/plant height in metres.
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Placement")]
    pub height_max: f32,
    /// Minimum width in metres.
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Placement")]
    pub width_min: f32,
    /// Maximum width in metres.
    #[property(min = 0.0, max = 1.0, step = 0.001, category = "Placement")]
    pub width_max: f32,
    /// Acceptance band lower edge on terrain slope, in degrees from horizontal.
    #[property(min = 0.0, max = 90.0, step = 0.5, category = "Placement")]
    pub slope_min_degrees: f32,
    /// Acceptance band upper edge on terrain slope, in degrees from horizontal.
    #[property(min = 0.0, max = 90.0, step = 0.5, category = "Placement")]
    pub slope_max_degrees: f32,
    /// Acceptance band lower edge on world altitude in metres.
    #[property(category = "Placement")]
    pub altitude_min: f32,
    /// Acceptance band upper edge on world altitude in metres.
    #[property(category = "Placement")]
    pub altitude_max: f32,
    /// Half-extent (metres) of the square layer the foliage grows in, centred on
    /// the owner object's XZ position.
    #[property(min = 1.0, max = 10000.0, step = 1.0, category = "Placement")]
    pub layer_extent: f32,
    /// When set, the layer bounds are not enforced: grass grows across the whole
    /// foliage ring around the camera instead of stopping at the volume box.
    #[property(category = "Placement")]
    pub has_infinite_extent: bool,
}

impl Default for PlacementFoliageProps {
    fn default() -> Self {
        Self {
            height_min: 0.18,
            height_max: 0.5,
            width_min: 0.012,
            width_max: 0.03,
            slope_min_degrees: 0.0,
            slope_max_degrees: 35.0,
            altitude_min: -10000.0,
            altitude_max: 10000.0,
            layer_extent: 120.0,
            has_infinite_extent: false,
        }
    }
}

impl PlacementFoliageProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("height_min").and_then(|v| v.as_f64()) {
            self.height_min = v as f32;
        }
        if let Some(v) = obj.get("height_max").and_then(|v| v.as_f64()) {
            self.height_max = v as f32;
        }
        if let Some(v) = obj.get("width_min").and_then(|v| v.as_f64()) {
            self.width_min = v as f32;
        }
        if let Some(v) = obj.get("width_max").and_then(|v| v.as_f64()) {
            self.width_max = v as f32;
        }
        if let Some(v) = obj.get("slope_min_degrees").and_then(|v| v.as_f64()) {
            self.slope_min_degrees = v as f32;
        }
        if let Some(v) = obj.get("slope_max_degrees").and_then(|v| v.as_f64()) {
            self.slope_max_degrees = v as f32;
        }
        if let Some(v) = obj.get("altitude_min").and_then(|v| v.as_f64()) {
            self.altitude_min = v as f32;
        }
        if let Some(v) = obj.get("altitude_max").and_then(|v| v.as_f64()) {
            self.altitude_max = v as f32;
        }
        if let Some(v) = obj.get("layer_extent").and_then(|v| v.as_f64()) {
            self.layer_extent = v as f32;
        }
        if let Some(v) = obj.get("has_infinite_extent").and_then(|v| v.as_bool()) {
            self.has_infinite_extent = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("height_min".to_string(), Value::from(self.height_min));
        out.insert("height_max".to_string(), Value::from(self.height_max));
        out.insert("width_min".to_string(), Value::from(self.width_min));
        out.insert("width_max".to_string(), Value::from(self.width_max));
        out.insert(
            "slope_min_degrees".to_string(),
            Value::from(self.slope_min_degrees),
        );
        out.insert(
            "slope_max_degrees".to_string(),
            Value::from(self.slope_max_degrees),
        );
        out.insert("altitude_min".to_string(), Value::from(self.altitude_min));
        out.insert("altitude_max".to_string(), Value::from(self.altitude_max));
        out.insert("layer_extent".to_string(), Value::from(self.layer_extent));
        out.insert(
            "has_infinite_extent".to_string(),
            Value::from(self.has_infinite_extent),
        );
    }
}
