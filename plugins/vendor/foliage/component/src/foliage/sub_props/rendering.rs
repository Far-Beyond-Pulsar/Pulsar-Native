use engine_class_derive::engine_class;
use serde_json::Value;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Rendering", category_color = "#FB7185", default_collapsed = true)]
pub struct RenderingFoliageProps {
    /// Render both faces. Almost always wanted for vegetation.
    #[property(category = "Rendering")]
    pub two_sided: bool,
    /// Contribute to the shadow atlas.
    #[property(category = "Rendering")]
    pub casts_shadow: bool,
    /// Distance of the first LOD transition, in metres.
    #[property(min = 0.0, max = 1000.0, step = 1.0, category = "Rendering")]
    pub lod_distance_0: f32,
    /// Distance of the second LOD transition, in metres.
    #[property(min = 0.0, max = 1000.0, step = 1.0, category = "Rendering")]
    pub lod_distance_1: f32,
    /// Distance of the third LOD transition, in metres.
    #[property(min = 0.0, max = 1000.0, step = 1.0, category = "Rendering")]
    pub lod_distance_2: f32,
    /// Distance of the fourth LOD transition, in metres.
    #[property(min = 0.0, max = 1000.0, step = 1.0, category = "Rendering")]
    pub lod_distance_3: f32,
    /// Base color (RGBA linear) used for the foliage material.
    #[property(category = "Rendering")]
    pub base_color: [f32; 4],
    /// Roughness of the foliage material (0 = smooth, 1 = rough/matte).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Rendering")]
    pub roughness: f32,
    /// Metallic factor of the foliage material (0 = dielectric, 1 = metal).
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Rendering")]
    pub metallic: f32,
    /// Each tuft takes the colour of the ground under it, times twice the
    /// base colour (0.5: the ground's own), so a meadow reads as its turf.
    #[serde(default)]
    #[property(category = "Rendering", label = "Ground colour")]
    pub ground_colour: bool,
}

impl Default for RenderingFoliageProps {
    fn default() -> Self {
        Self {
            two_sided: true,
            casts_shadow: false,
            lod_distance_0: 8.0,
            lod_distance_1: 20.0,
            lod_distance_2: 45.0,
            lod_distance_3: 120.0,
            base_color: [0.28, 0.46, 0.14, 1.0],
            roughness: 0.85,
            metallic: 0.0,
            ground_colour: false,
        }
    }
}

impl RenderingFoliageProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("two_sided").and_then(|v| v.as_bool()) {
            self.two_sided = v;
        }
        if let Some(v) = obj.get("casts_shadow").and_then(|v| v.as_bool()) {
            self.casts_shadow = v;
        }
        if let Some(v) = obj.get("lod_distance_0").and_then(|v| v.as_f64()) {
            self.lod_distance_0 = v as f32;
        }
        if let Some(v) = obj.get("lod_distance_1").and_then(|v| v.as_f64()) {
            self.lod_distance_1 = v as f32;
        }
        if let Some(v) = obj.get("lod_distance_2").and_then(|v| v.as_f64()) {
            self.lod_distance_2 = v as f32;
        }
        if let Some(v) = obj.get("lod_distance_3").and_then(|v| v.as_f64()) {
            self.lod_distance_3 = v as f32;
        }
        if let Some(arr) = obj.get("base_color").and_then(|v| v.as_array())
            && arr.len() >= 4
        {
            self.base_color = [
                arr[0].as_f64().unwrap_or(1.0) as f32,
                arr[1].as_f64().unwrap_or(1.0) as f32,
                arr[2].as_f64().unwrap_or(1.0) as f32,
                arr[3].as_f64().unwrap_or(1.0) as f32,
            ];
        }
        if let Some(v) = obj.get("roughness").and_then(|v| v.as_f64()) {
            self.roughness = v as f32;
        }
        if let Some(v) = obj.get("metallic").and_then(|v| v.as_f64()) {
            self.metallic = v as f32;
        }
    }
}
