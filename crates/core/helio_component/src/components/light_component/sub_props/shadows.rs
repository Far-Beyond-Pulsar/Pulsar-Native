use engine_class_derive::engine_class;
use serde_json::Value;

/// `#[gpu(as = u32, with = ...)]` target for `cast_shadows` -- encodes the
/// "does this light request shadows at all" flag as `helio::GpuLight::
/// shadow_index`'s own request/disabled sentinel (`0` = requests shadows,
/// `u32::MAX` = explicitly disabled), computed once at mirror-build time.
///
/// Not the FINAL shadow atlas slot: `helio-pass-shadow-matrix`'s GPU caster
/// allocation (`shadow_casters.wgsl`, Helio#246) overwrites it in the light's
/// GPU row with the real assigned slot, or `u32::MAX` if the light loses the
/// budget, whenever the light rows change. That value depends on every other
/// live light's score, so it cannot be computed per entity at upload time.
/// This function only encodes the request.
pub fn cast_shadows_to_shadow_request(cast_shadows: bool) -> u32 {
    if cast_shadows { 0 } else { u32::MAX }
}

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Shadows", category_color = "#A78BFA", default_collapsed = true)]
// Fields added later load from levels saved before them.
#[serde(default)]
pub struct ShadowLightProps {
    #[property(category = "Shadows")]
    #[gpu(as = u32, with = cast_shadows_to_shadow_request)]
    pub cast_shadows: bool,
    #[property(category = "Shadows")]
    #[gpu(as = u32, with = shadow_bool)]
    pub cast_static_shadows: bool,
    #[property(category = "Shadows")]
    #[gpu(as = u32, with = shadow_bool)]
    pub cast_dynamic_shadows: bool,
    #[property(category = "Shadows")]
    #[gpu(as = f32, with = volumetric_shadow_strength)]
    pub cast_volumetric_shadow: bool,
    #[property(category = "Shadows")]
    #[gpu(as = u32, with = shadow_bool)]
    pub cast_contact_shadows: bool,
    #[property(min = 0.0, max = 10.0, step = 0.01, category = "Shadows")]
    pub shadow_bias: f32,
    #[property(min = 0.0625, max = 15.9375, step = 0.0625, category = "Shadows")]
    #[gpu]
    pub shadow_priority: f32,
    #[property(min = 128, max = 2048, category = "Shadows")]
    #[gpu]
    pub shadow_max_resolution: u32,
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

pub fn volumetric_shadow_strength(enabled: bool) -> f32 {
    if enabled { 1.0 } else { 0.0 }
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
            shadow_priority: 1.0,
            shadow_max_resolution: 2048,
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
        if let Some(v) = obj.get("shadow_priority").and_then(Value::as_f64) {
            self.shadow_priority = v as f32;
        }
        if let Some(v) = obj.get("shadow_max_resolution").and_then(Value::as_u64) {
            self.shadow_max_resolution = v.min(2048).max(128) as u32;
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
}

pub fn shadow_bool(value: bool) -> u32 {
    u32::from(value)
}
