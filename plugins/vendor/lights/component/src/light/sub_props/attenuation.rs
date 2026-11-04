use engine_class_derive::engine_class;
use serde_json::Value;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Attenuation", category_color = "#6EC5FF")]
// Fields added later load from levels saved before them.
#[serde(default)]
pub struct AttenuationLightProps {
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Attenuation")]
    #[gpu]
    pub range: f32,
    #[property(min = 0.0, max = 5000.0, step = 1.0, category = "Attenuation")]
    pub falloff_start: f32,
    #[property(min = 0.1, max = 16.0, step = 0.1, category = "Attenuation")]
    pub attenuation_exponent: f32,
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Attenuation")]
    pub source_radius: f32,
    #[property(min = 0.0, max = 200.0, step = 0.1, category = "Attenuation")]
    pub source_length: f32,
    // `as = f32, with = cone_degrees_to_cosine`: the properties panel edits
    // degrees (human-friendly); the GPU mirror holds the COSINE of the
    // angle -- what `GpuLight`'s contract actually specifies for these two
    // fields (`direction_outer.w` = spot outer-cos, `inner_angle` = spot
    // inner-cos: every lighting shader smoothsteps them against
    // `dot(-L, direction)` directly). Uploading raw radians instead made
    // that smoothstep run reversed (edge0 > edge1) -- a dark disc at the
    // spot's center with the bright ring outside it (#172). Computed once
    // here, at mirror-build time, not by a hand-written post-mirror
    // translation function. See `pulsar_world_registry`'s `GpuMirrored` doc
    // / `engine_class_derive`'s `GpuFieldOverride` doc for why this belongs
    // on the field itself.
    #[property(min = 0.0, max = 90.0, step = 1.0, category = "Attenuation")]
    #[gpu(as = f32, with = cone_degrees_to_cosine)]
    pub inner_cone_angle: f32,
    #[property(min = 0.0, max = 90.0, step = 1.0, category = "Attenuation")]
    #[gpu(as = f32, with = cone_degrees_to_cosine)]
    pub outer_cone_angle: f32,
}

/// `#[gpu(with = ...)]` target for both cone-angle fields -- degrees as the
/// human edits them to cosines as `GpuLight` stores them (see the fields'
/// own comment for why the cosine is the right upload shape).
fn cone_degrees_to_cosine(degrees: f32) -> f32 {
    degrees.to_radians().cos()
}

impl Default for AttenuationLightProps {
    fn default() -> Self {
        Self {
            range: 1000.0,
            falloff_start: 0.0,
            attenuation_exponent: 2.0,
            source_radius: 0.0,
            source_length: 0.0,
            inner_cone_angle: 30.0,
            outer_cone_angle: 45.0,
        }
    }
}

impl AttenuationLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("range").and_then(|v| v.as_f64()) {
            self.range = v as f32;
        }
        if let Some(v) = obj.get("falloff_start").and_then(|v| v.as_f64()) {
            self.falloff_start = v as f32;
        }
        if let Some(v) = obj.get("attenuation_exponent").and_then(|v| v.as_f64()) {
            self.attenuation_exponent = v as f32;
        }
        if let Some(v) = obj.get("source_radius").and_then(|v| v.as_f64()) {
            self.source_radius = v as f32;
        }
        if let Some(v) = obj.get("source_length").and_then(|v| v.as_f64()) {
            self.source_length = v as f32;
        }
        if let Some(v) = obj.get("inner_cone_angle").and_then(|v| v.as_f64()) {
            self.inner_cone_angle = v as f32;
        }
        if let Some(v) = obj.get("outer_cone_angle").and_then(|v| v.as_f64()) {
            self.outer_cone_angle = v as f32;
        }
    }
}
