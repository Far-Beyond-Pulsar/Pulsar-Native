use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("General", category_color = "#4ADE80")]
pub struct GeneralProceduralTerrainProps {
    #[property(category = "General")]
    pub enabled: bool,
    #[property(category = "General")]
    pub script_path: String,
    #[property(category = "General")]
    pub script_function: String,
    #[property(min = 0, max = 2147483647, step = 1, category = "General")]
    pub seed: u64,
    #[property(min = 0.1, max = 1000.0, step = 0.1, category = "General")]
    pub voxel_size: f32,
    #[property(min = 8, max = 128, step = 8, category = "General")]
    pub chunk_size: u64,
    #[property(min = 1, max = 64, step = 1, category = "General")]
    pub render_distance: u64,
}

impl Default for GeneralProceduralTerrainProps {
    fn default() -> Self {
        Self {
            enabled: true,
            script_path: String::new(),
            script_function: "generate_terrain".into(),
            seed: 42,
            voxel_size: 100.0,
            chunk_size: 32,
            render_distance: 8,
        }
    }
}

impl GeneralProceduralTerrainProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("enabled").and_then(|v| v.as_bool()) {
            self.enabled = v;
        }
        if let Some(v) = obj.get("script_path").and_then(|v| v.as_str()) {
            self.script_path = v.to_string();
        }
        if let Some(v) = obj.get("script_function").and_then(|v| v.as_str()) {
            self.script_function = v.to_string();
        }
        if let Some(v) = obj.get("seed").and_then(|v| v.as_u64()) {
            self.seed = v;
        }
        if let Some(v) = obj.get("voxel_size").and_then(|v| v.as_f64()).map(|v| v as f32) {
            self.voxel_size = v;
        }
        if let Some(v) = obj.get("chunk_size").and_then(|v| v.as_u64()) {
            self.chunk_size = v;
        }
        if let Some(v) = obj.get("render_distance").and_then(|v| v.as_u64()) {
            self.render_distance = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert("enabled".to_string(), Value::from(self.enabled));
        out.insert("script_path".to_string(), Value::from(self.script_path.clone()));
        out.insert("script_function".to_string(), Value::from(self.script_function.clone()));
        out.insert("seed".to_string(), Value::from(self.seed));
        out.insert("voxel_size".to_string(), Value::from(self.voxel_size));
        out.insert("chunk_size".to_string(), Value::from(self.chunk_size));
        out.insert("render_distance".to_string(), Value::from(self.render_distance));
    }
}
