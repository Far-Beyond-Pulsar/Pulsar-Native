use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq, pulsar_reflection::Reflectable)]
pub enum NoiseType {
    Perlin,
    Simplex,
    Cellular,
    Ridged,
}

impl Default for NoiseType {
    fn default() -> Self { Self::Perlin }
}

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Generation", category_color = "#818CF8", default_collapsed = false)]
pub struct GenerationTerrainProps {
    #[property(category = "Generation")]
    pub noise_type: NoiseType,
    #[property(min = 1, max = 16, step = 1, category = "Generation")]
    pub octaves: u64,
    #[property(min = 0.5, max = 4.0, step = 0.1, category = "Generation")]
    pub lacunarity: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Generation")]
    pub persistence: f32,
    #[property(min = 0.0, max = 10000.0, step = 10.0, category = "Generation")]
    pub base_height: f32,
    #[property(min = 0.0, max = 10000.0, step = 10.0, category = "Generation")]
    pub amplitude: f32,
    #[property(min = -5000.0, max = 5000.0, step = 10.0, category = "Generation")]
    pub height_offset: f32,
}

impl Default for GenerationTerrainProps {
    fn default() -> Self {
        Self {
            noise_type: NoiseType::Perlin,
            octaves: 6,
            lacunarity: 2.0,
            persistence: 0.5,
            base_height: 0.0,
            amplitude: 500.0,
            height_offset: 0.0,
        }
    }
}

impl GenerationTerrainProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(ix) = obj.get("noise_type").and_then(|v| v.as_u64()) {
            self.noise_type = match ix {
                0 => NoiseType::Perlin,
                1 => NoiseType::Simplex,
                2 => NoiseType::Cellular,
                3 => NoiseType::Ridged,
                _ => self.noise_type,
            };
        }
        if let Some(v) = obj.get("octaves").and_then(|v| v.as_u64()) { self.octaves = v; }
        if let Some(v) = obj.get("lacunarity").and_then(|v| v.as_f64()).map(|v| v as f32) { self.lacunarity = v; }
        if let Some(v) = obj.get("persistence").and_then(|v| v.as_f64()).map(|v| v as f32) { self.persistence = v; }
        if let Some(v) = obj.get("base_height").and_then(|v| v.as_f64()).map(|v| v as f32) { self.base_height = v; }
        if let Some(v) = obj.get("amplitude").and_then(|v| v.as_f64()).map(|v| v as f32) { self.amplitude = v; }
        if let Some(v) = obj.get("height_offset").and_then(|v| v.as_f64()).map(|v| v as f32) { self.height_offset = v; }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("noise_type".to_string(), Value::from(self.noise_type as u64));
        out.insert("octaves".to_string(), Value::from(self.octaves));
        out.insert("lacunarity".to_string(), Value::from(self.lacunarity));
        out.insert("persistence".to_string(), Value::from(self.persistence));
        out.insert("base_height".to_string(), Value::from(self.base_height));
        out.insert("amplitude".to_string(), Value::from(self.amplitude));
        out.insert("height_offset".to_string(), Value::from(self.height_offset));
    }
}
