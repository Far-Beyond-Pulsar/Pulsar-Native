use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::VoxelMaterialSource;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Material", category_color = "#F97316", default_collapsed = false)]
pub struct MaterialTerrainProps {
    #[property(category = "Material")]
    pub source: VoxelMaterialSource,
    #[property(category = "Material")]
    pub palette_texture: String,
    #[property(category = "Material")]
    pub base_color: [f32; 4],
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Material")]
    pub roughness: f32,
    #[property(min = 0.0, max = 1.0, step = 0.01, category = "Material")]
    pub metallic: f32,
}

impl Default for MaterialTerrainProps {
    fn default() -> Self {
        Self {
            source: VoxelMaterialSource::Single,
            palette_texture: String::new(),
            base_color: [0.5, 0.5, 0.4, 1.0],
            roughness: 0.8,
            metallic: 0.0,
        }
    }
}

impl MaterialTerrainProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(ix) = obj.get("voxel_material_source").and_then(|v| v.as_u64()) {
            self.source = match ix {
                0 => VoxelMaterialSource::Single,
                1 => VoxelMaterialSource::Palette,
                2 => VoxelMaterialSource::Texture,
                _ => self.source,
            };
        }
        if let Some(v) = obj.get("palette_texture").and_then(|v| v.as_str()) {
            self.palette_texture = v.to_string();
        }
        if let Some(arr) = obj.get("base_color").and_then(|v| v.as_array()) {
            for (i, v) in arr.iter().enumerate().take(4) {
                if let Some(n) = v.as_f64() {
                    self.base_color[i] = n as f32;
                }
            }
        }
        if let Some(v) = obj.get("roughness").and_then(|v| v.as_f64()).map(|v| v as f32) {
            self.roughness = v;
        }
        if let Some(v) = obj.get("metallic").and_then(|v| v.as_f64()).map(|v| v as f32) {
            self.metallic = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "voxel_material_source".to_string(),
            Value::from(self.source as u64),
        );
        out.insert(
            "palette_texture".to_string(),
            Value::from(self.palette_texture.clone()),
        );
        out.insert(
            "base_color".to_string(),
            Value::from(Vec::from(self.base_color)),
        );
        out.insert("roughness".to_string(), Value::from(self.roughness));
        out.insert("metallic".to_string(), Value::from(self.metallic));
    }
}
