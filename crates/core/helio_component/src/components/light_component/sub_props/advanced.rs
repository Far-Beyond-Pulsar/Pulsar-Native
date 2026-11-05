use engine_class_derive::engine_class;
use serde_json::Value;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Advanced", category_color = "#9CA3AF", default_collapsed = true)]
// Fields added later load from levels saved before them.
#[serde(default)]
pub struct AdvancedLightProps {
    /// Per-light Euler rotation offset in degrees. This lets multiple lights
    /// attached to one object keep independent directions while still
    /// inheriting the object's world rotation.
    #[property(category = "Advanced")]
    pub rotation_offset: [f32; 3],
    /// Prefer this directional light for atmosphere, water, and other systems
    /// that need one primary sky light. Other directional lights still render.
    #[property(category = "Advanced")]
    pub primary_atmosphere_light: bool,
    #[property(category = "Advanced")]
    pub affects_translucency: bool,
    #[property(category = "Advanced")]
    pub affects_reflections: bool,
    #[property(category = "Advanced")]
    pub affects_global_illumination: bool,
    #[property(min = 0.0, max = 8.0, step = 0.01, category = "Advanced")]
    pub specular_scale: f32,
    #[property(min = 0.0, max = 8.0, step = 0.01, category = "Advanced")]
    pub diffuse_scale: f32,
}

impl Default for AdvancedLightProps {
    fn default() -> Self {
        Self {
            rotation_offset: [0.0; 3],
            primary_atmosphere_light: false,
            affects_translucency: true,
            affects_reflections: true,
            affects_global_illumination: true,
            specular_scale: 1.0,
            diffuse_scale: 1.0,
        }
    }
}

impl AdvancedLightProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(arr) = obj.get("rotation_offset").and_then(|v| v.as_array())
            && arr.len() >= 3
        {
            self.rotation_offset = [
                arr[0].as_f64().unwrap_or(0.0) as f32,
                arr[1].as_f64().unwrap_or(0.0) as f32,
                arr[2].as_f64().unwrap_or(0.0) as f32,
            ];
        }
        if let Some(v) = obj
            .get("primary_atmosphere_light")
            .and_then(|v| v.as_bool())
        {
            self.primary_atmosphere_light = v;
        }
        if let Some(v) = obj.get("affects_translucency").and_then(|v| v.as_bool()) {
            self.affects_translucency = v;
        }
        if let Some(v) = obj.get("affects_reflections").and_then(|v| v.as_bool()) {
            self.affects_reflections = v;
        }
        if let Some(v) = obj
            .get("affects_global_illumination")
            .and_then(|v| v.as_bool())
        {
            self.affects_global_illumination = v;
        }
        if let Some(v) = obj.get("specular_scale").and_then(|v| v.as_f64()) {
            self.specular_scale = v as f32;
        }
        if let Some(v) = obj.get("diffuse_scale").and_then(|v| v.as_f64()) {
            self.diffuse_scale = v as f32;
        }
    }
}
