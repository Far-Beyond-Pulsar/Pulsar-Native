use engine_class_derive::engine_class;
use serde_json::Value;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Volumetrics", category_color = "#7EE787", default_collapsed = true)]
// Fields added later load from levels saved before them.
#[serde(default)]
pub struct VolumetricLightProps {
    #[property(category = "Volumetrics")]
    #[gpu(as = u32, with = volumetric_enabled_to_gpu)]
    pub affects_volumetric_fog: bool,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    #[gpu]
    pub volumetric_scattering_intensity: f32,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    pub volumetric_shadow_intensity: f32,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    #[gpu]
    pub fog_inscattering_intensity: f32,
    #[property(min = 0.0, max = 50.0, step = 0.1, category = "Volumetrics")]
    pub contact_shadow_length: f32,
}

pub fn volumetric_enabled_to_gpu(enabled: bool) -> u32 {
    u32::from(enabled)
}

impl Default for VolumetricLightProps {
    fn default() -> Self {
        Self {
            affects_volumetric_fog: true,
            volumetric_scattering_intensity: 1.0,
            volumetric_shadow_intensity: 1.0,
            fog_inscattering_intensity: 1.0,
            contact_shadow_length: 0.0,
        }
    }
}

impl VolumetricLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("affects_volumetric_fog").and_then(|v| v.as_bool()) {
            self.affects_volumetric_fog = v;
        }
        if let Some(v) = obj
            .get("volumetric_scattering_intensity")
            .and_then(|v| v.as_f64())
        {
            self.volumetric_scattering_intensity = v as f32;
        }
        if let Some(v) = obj
            .get("volumetric_shadow_intensity")
            .and_then(|v| v.as_f64())
        {
            self.volumetric_shadow_intensity = v as f32;
        }
        if let Some(v) = obj
            .get("fog_inscattering_intensity")
            .and_then(|v| v.as_f64())
        {
            self.fog_inscattering_intensity = v as f32;
        }
        if let Some(v) = obj.get("contact_shadow_length").and_then(|v| v.as_f64()) {
            self.contact_shadow_length = v as f32;
        }
    }
}
