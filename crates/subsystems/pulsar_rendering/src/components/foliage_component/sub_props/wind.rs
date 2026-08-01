use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Wind", category_color = "#7EE787")]
pub struct WindFoliageProps {
    /// Per-band wind gain: trunk sway.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Wind")]
    pub trunk_sway: f32,
    /// Per-band wind gain: branch flutter.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Wind")]
    pub branch_flutter: f32,
    /// Per-band wind gain: leaf jitter.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Wind")]
    pub leaf_jitter: f32,
    /// How fast a bent plant recovers. Larger is stiffer.
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Wind")]
    pub interaction_stiffness: f32,
    /// When true this component drives the scene's global wind.
    #[property(category = "Wind")]
    pub wind_enabled: bool,
    /// World-space wind direction (need not be normalised).
    #[property(category = "Wind")]
    pub wind_direction: [f32; 3],
    /// Base wind speed in m/s.
    #[property(min = 0.0, max = 30.0, step = 0.1, category = "Wind")]
    pub wind_speed: f32,
    /// Peak additional sway during a gust, as a multiple of the base amplitude.
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Wind")]
    pub gust_amplitude: f32,
    /// Gust rate in Hz.
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Wind")]
    pub gust_frequency: f32,
    /// Spatial frequency of the gust noise in 1/m.
    #[property(min = 0.0, max = 2.0, step = 0.01, category = "Wind")]
    pub turbulence_scale: f32,
}

impl Default for WindFoliageProps {
    fn default() -> Self {
        Self {
            trunk_sway: 0.0,
            branch_flutter: 0.35,
            leaf_jitter: 1.0,
            interaction_stiffness: 6.0,
            wind_enabled: true,
            wind_direction: [1.0, 0.0, 0.35],
            wind_speed: 2.0,
            gust_amplitude: 0.6,
            gust_frequency: 0.25,
            turbulence_scale: 0.05,
        }
    }
}

impl WindFoliageProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("trunk_sway").and_then(|v| v.as_f64()) {
            self.trunk_sway = v as f32;
        }
        if let Some(v) = obj.get("branch_flutter").and_then(|v| v.as_f64()) {
            self.branch_flutter = v as f32;
        }
        if let Some(v) = obj.get("leaf_jitter").and_then(|v| v.as_f64()) {
            self.leaf_jitter = v as f32;
        }
        if let Some(v) = obj.get("interaction_stiffness").and_then(|v| v.as_f64()) {
            self.interaction_stiffness = v as f32;
        }
        if let Some(v) = obj.get("wind_enabled").and_then(|v| v.as_bool()) {
            self.wind_enabled = v;
        }
        if let Some(arr) = obj.get("wind_direction").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.wind_direction = [
                arr[0].as_f64().unwrap_or(1.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(v) = obj.get("wind_speed").and_then(|v| v.as_f64()) {
            self.wind_speed = v as f32;
        }
        if let Some(v) = obj.get("gust_amplitude").and_then(|v| v.as_f64()) {
            self.gust_amplitude = v as f32;
        }
        if let Some(v) = obj.get("gust_frequency").and_then(|v| v.as_f64()) {
            self.gust_frequency = v as f32;
        }
        if let Some(v) = obj.get("turbulence_scale").and_then(|v| v.as_f64()) {
            self.turbulence_scale = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("trunk_sway".to_string(), Value::from(self.trunk_sway));
        out.insert(
            "branch_flutter".to_string(),
            Value::from(self.branch_flutter),
        );
        out.insert("leaf_jitter".to_string(), Value::from(self.leaf_jitter));
        out.insert(
            "interaction_stiffness".to_string(),
            Value::from(self.interaction_stiffness),
        );
        out.insert("wind_enabled".to_string(), Value::from(self.wind_enabled));
        out.insert(
            "wind_direction".to_string(),
            serde_json::json!([
                self.wind_direction[0],
                self.wind_direction[1],
                self.wind_direction[2]
            ]),
        );
        out.insert("wind_speed".to_string(), Value::from(self.wind_speed));
        out.insert(
            "gust_amplitude".to_string(),
            Value::from(self.gust_amplitude),
        );
        out.insert(
            "gust_frequency".to_string(),
            Value::from(self.gust_frequency),
        );
        out.insert(
            "turbulence_scale".to_string(),
            Value::from(self.turbulence_scale),
        );
    }
}
