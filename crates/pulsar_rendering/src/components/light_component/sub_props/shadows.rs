use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(clone, debug, serialize, deserialize)]
#[category("Shadows", category_color = "#A78BFA", default_collapsed = true)]
pub struct ShadowLightProps {
    #[property(category = "Shadows")]
    pub cast_shadows: bool,
    #[property(category = "Shadows")]
    pub cast_static_shadows: bool,
    #[property(category = "Shadows")]
    pub cast_dynamic_shadows: bool,
    #[property(category = "Shadows")]
    pub cast_volumetric_shadow: bool,
    #[property(category = "Shadows")]
    pub cast_contact_shadows: bool,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Shadows")]
    pub shadow_bias: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Shadows")]
    pub shadow_normal_bias: f32,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Shadows")]
    pub shadow_slope_bias: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Shadows")]
    pub shadow_filter_sharpen: f32,
    #[property(min = 0.0, max = 10.0, step = 0.05, category = "Shadows")]
    pub shadow_softness: f32,
    #[property(min = 0.25, max = 4.0, step = 0.05, category = "Shadows")]
    pub shadow_resolution_scale: f32,
    #[property(min = 0.0, max = 5.0, step = 0.01, category = "Shadows")]
    pub contact_shadow_non_shadow_casting_intensity: f32,
}

impl Default for ShadowLightProps {
    fn default() -> Self {
        Self {
            cast_shadows: true,
            cast_static_shadows: true,
            cast_dynamic_shadows: true,
            cast_volumetric_shadow: true,
            cast_contact_shadows: false,
            shadow_bias: 0.5,
            shadow_normal_bias: 0.5,
            shadow_slope_bias: 0.5,
            shadow_filter_sharpen: 0.0,
            shadow_softness: 1.0,
            shadow_resolution_scale: 1.0,
            contact_shadow_non_shadow_casting_intensity: 0.0,
        }
    }
}

impl ShadowLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("cast_shadows").and_then(|v| v.as_bool()) {
            self.cast_shadows = v;
        }
        if let Some(v) = obj.get("cast_static_shadows").and_then(|v| v.as_bool()) {
            self.cast_static_shadows = v;
        }
        if let Some(v) = obj.get("cast_dynamic_shadows").and_then(|v| v.as_bool()) {
            self.cast_dynamic_shadows = v;
        }
        if let Some(v) = obj.get("cast_volumetric_shadow").and_then(|v| v.as_bool()) {
            self.cast_volumetric_shadow = v;
        }
        if let Some(v) = obj.get("cast_contact_shadows").and_then(|v| v.as_bool()) {
            self.cast_contact_shadows = v;
        }
        if let Some(v) = obj.get("shadow_bias").and_then(|v| v.as_f64()) {
            self.shadow_bias = v as f32;
        }
        if let Some(v) = obj.get("shadow_normal_bias").and_then(|v| v.as_f64()) {
            self.shadow_normal_bias = v as f32;
        }
        if let Some(v) = obj.get("shadow_slope_bias").and_then(|v| v.as_f64()) {
            self.shadow_slope_bias = v as f32;
        }
        if let Some(v) = obj.get("shadow_filter_sharpen").and_then(|v| v.as_f64()) {
            self.shadow_filter_sharpen = v as f32;
        }
        if let Some(v) = obj.get("shadow_softness").and_then(|v| v.as_f64()) {
            self.shadow_softness = v as f32;
        }
        if let Some(v) = obj.get("shadow_resolution_scale").and_then(|v| v.as_f64()) {
            self.shadow_resolution_scale = v as f32;
        }
        if let Some(v) = obj
            .get("contact_shadow_non_shadow_casting_intensity")
            .and_then(|v| v.as_f64())
        {
            self.contact_shadow_non_shadow_casting_intensity = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("cast_shadows".to_string(), Value::from(self.cast_shadows));
        out.insert(
            "cast_static_shadows".to_string(),
            Value::from(self.cast_static_shadows),
        );
        out.insert(
            "cast_dynamic_shadows".to_string(),
            Value::from(self.cast_dynamic_shadows),
        );
        out.insert(
            "cast_volumetric_shadow".to_string(),
            Value::from(self.cast_volumetric_shadow),
        );
        out.insert(
            "cast_contact_shadows".to_string(),
            Value::from(self.cast_contact_shadows),
        );
        out.insert("shadow_bias".to_string(), Value::from(self.shadow_bias));
        out.insert(
            "shadow_normal_bias".to_string(),
            Value::from(self.shadow_normal_bias),
        );
        out.insert(
            "shadow_slope_bias".to_string(),
            Value::from(self.shadow_slope_bias),
        );
        out.insert(
            "shadow_filter_sharpen".to_string(),
            Value::from(self.shadow_filter_sharpen),
        );
        out.insert(
            "shadow_softness".to_string(),
            Value::from(self.shadow_softness),
        );
        out.insert(
            "shadow_resolution_scale".to_string(),
            Value::from(self.shadow_resolution_scale),
        );
        out.insert(
            "contact_shadow_non_shadow_casting_intensity".to_string(),
            Value::from(self.contact_shadow_non_shadow_casting_intensity),
        );
    }
}
