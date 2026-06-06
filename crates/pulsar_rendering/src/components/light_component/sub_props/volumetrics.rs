use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Volumetrics", category_color = "#7EE787", default_collapsed = true)]
pub struct VolumetricLightProps {
    #[property(category = "Volumetrics")]
    pub affects_volumetric_fog: bool,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    pub volumetric_scattering_intensity: f32,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    pub volumetric_shadow_intensity: f32,
    #[property(min = 0.0, max = 8.0, step = 0.05, category = "Volumetrics")]
    pub fog_inscattering_intensity: f32,
    #[property(min = 0.0, max = 50.0, step = 0.1, category = "Volumetrics")]
    pub contact_shadow_length: f32,
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
        if let Some(v) = obj.get("fog_inscattering_intensity").and_then(|v| v.as_f64()) {
            self.fog_inscattering_intensity = v as f32;
        }
        if let Some(v) = obj.get("contact_shadow_length").and_then(|v| v.as_f64()) {
            self.contact_shadow_length = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "affects_volumetric_fog".to_string(),
            Value::from(self.affects_volumetric_fog),
        );
        out.insert(
            "volumetric_scattering_intensity".to_string(),
            Value::from(self.volumetric_scattering_intensity),
        );
        out.insert(
            "volumetric_shadow_intensity".to_string(),
            Value::from(self.volumetric_shadow_intensity),
        );
        out.insert(
            "fog_inscattering_intensity".to_string(),
            Value::from(self.fog_inscattering_intensity),
        );
        out.insert(
            "contact_shadow_length".to_string(),
            Value::from(self.contact_shadow_length),
        );
    }
}
