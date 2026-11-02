use engine_class_derive::engine_class;
use serde_json::Value;

use super::super::{LightType, light_type_to_gpu_u32};
use crate::components::ObjectMovability;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#F4C542")]
// Fields added later load from levels saved before them.
#[serde(default)]
pub struct GeneralLightProps {
    /// Uploaded as 0/1: a disabled light keeps its GPU row, marked absent,
    /// so its presence is data rather than whether a row exists.
    #[property(category = "General")]
    #[gpu(as = u32, with = enabled_to_gpu_u32)]
    pub enabled: bool,
    #[property(category = "General")]
    pub affects_world: bool,
    // See `light_type_to_gpu_u32`'s own doc: a genuine semantic remap
    // (Helio has no Area light type), computed once at mirror-build time.
    #[property(category = "General")]
    #[gpu(as = u32, with = light_type_to_gpu_u32)]
    pub light_type: LightType,
    /// See [`ObjectMovability`] (Pulsar-Native#837); read through
    /// [`crate::components::object_movability`].
    #[property(category = "General")]
    #[serde(default)]
    pub movability: ObjectMovability,
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
            movability: ObjectMovability::Static,
            light_channels: 0xFF,
            lighting_channel_0: true,
            lighting_channel_1: false,
            lighting_channel_2: false,
        }
    }
}

fn enabled_to_gpu_u32(enabled: bool) -> u32 {
    u32::from(enabled)
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
        if let Some(v) = obj
            .get("movability")
            .and_then(|v| serde_json::from_value::<ObjectMovability>(v.clone()).ok())
        {
            self.movability = v;
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
}
