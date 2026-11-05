use engine_class_derive::engine_class;
use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The built-in sky emitter textures currently available to a LightComponent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Reflectable)]
pub enum EmitterDiskTexture {
    /// Render a solid disk using the light's color and intensity.
    #[default]
    None,
    /// Bundled Solar System Scope 2K lunar albedo, licensed CC BY 4.0.
    MoonSolarSystemScopeCCBY40,
}

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Emitter Disk", category_color = "#D5C9FF")]
#[serde(default)]
pub struct EmitterDiskProps {
    #[property(category = "Emitter Disk")]
    pub enabled: bool,
    /// Choose the image on the directional light's visible sky disk. The moon
    /// image is bundled from Solar System Scope and licensed CC BY 4.0.
    #[property(category = "Emitter Disk")]
    pub texture: EmitterDiskTexture,
    /// Angular radius in degrees; changing this scales both the disk and its
    /// texture. With texture set to None, it scales a solid emitter disk.
    #[property(min = 0.01, max = 10.0, step = 0.01, category = "Emitter Disk")]
    pub angular_radius_degrees: f32,
}

impl Default for EmitterDiskProps {
    fn default() -> Self {
        Self {
            enabled: false,
            texture: EmitterDiskTexture::None,
            // Slightly oversized for a clear, readable default moon disk.
            angular_radius_degrees: 2.5,
        }
    }
}

impl EmitterDiskProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(enabled) = obj.get("enabled").and_then(Value::as_bool) {
            self.enabled = enabled;
        }
        if let Some(texture) = obj.get("texture").and_then(Value::as_str) {
            self.texture = match texture {
                "MoonSolarSystemScopeCCBY40" => EmitterDiskTexture::MoonSolarSystemScopeCCBY40,
                "None" => EmitterDiskTexture::None,
                _ => self.texture,
            };
        }
        if let Some(radius) = obj.get("angular_radius_degrees").and_then(Value::as_f64) {
            self.angular_radius_degrees = radius as f32;
        }
    }
}
