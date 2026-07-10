use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::LightType;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
pub struct GeneralLightProps {
    #[property(category = "General")]
    pub enabled: bool,
    #[property(category = "General")]
    pub affects_world: bool,
    #[property(category = "General")]
    pub light_type: LightType,
    #[property(min = 0.0, max = 255.0, step = 1.0, category = "General")]
    pub light_channels: u64,
    #[property(category = "General")]
    pub lighting_channel_0: bool,
    #[property(category = "General")]
    pub lighting_channel_1: bool,
    #[property(category = "General")]
    pub lighting_channel_2: bool,
}

impl Default for GeneralLightProps {
    fn default() -> Self {
        Self {
            enabled: true,
            affects_world: true,
            light_type: LightType::Point,
            light_channels: 0xFF,
            lighting_channel_0: true,
            lighting_channel_1: false,
            lighting_channel_2: false,
        }
    }
}

impl GeneralLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("enabled").and_then(|v| v.as_bool()) {
            self.enabled = v;
        }
        if let Some(v) = obj.get("affects_world").and_then(|v| v.as_bool()) {
            self.affects_world = v;
        }
        if let Some(ix) = obj.get("light_type").and_then(|v| v.as_u64()) {
            self.light_type = match ix {
                0 => LightType::Directional,
                1 => LightType::Point,
                2 => LightType::Spot,
                3 => LightType::Area,
                _ => self.light_type,
            };
        }
        if let Some(v) = obj.get("light_channels").and_then(|v| v.as_u64()) {
            self.light_channels = v;
        }
        if let Some(v) = obj.get("lighting_channel_0").and_then(|v| v.as_bool()) {
            self.lighting_channel_0 = v;
        }
        if let Some(v) = obj.get("lighting_channel_1").and_then(|v| v.as_bool()) {
            self.lighting_channel_1 = v;
        }
        if let Some(v) = obj.get("lighting_channel_2").and_then(|v| v.as_bool()) {
            self.lighting_channel_2 = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("enabled".to_string(), Value::from(self.enabled));
        out.insert("affects_world".to_string(), Value::from(self.affects_world));
        out.insert(
            "light_type".to_string(),
            Value::from(self.light_type as u64),
        );
        out.insert(
            "light_channels".to_string(),
            Value::from(self.light_channels),
        );
        out.insert(
            "lighting_channel_0".to_string(),
            Value::from(self.lighting_channel_0),
        );
        out.insert(
            "lighting_channel_1".to_string(),
            Value::from(self.lighting_channel_1),
        );
        out.insert(
            "lighting_channel_2".to_string(),
            Value::from(self.lighting_channel_2),
        );
    }
}
