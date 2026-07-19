use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

use super::super::MeshingAlgorithm;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Rendering", category_color = "#22D3EE", default_collapsed = true)]
pub struct RenderingTerrainProps {
    #[property(category = "Rendering")]
    pub meshing_algorithm: MeshingAlgorithm,
    #[property(category = "Rendering")]
    pub enable_lod: bool,
    #[property(min = 1, max = 8, step = 1, category = "Rendering")]
    pub lod_levels: u64,
    #[property(category = "Rendering")]
    pub enable_collision: bool,
    #[property(category = "Rendering")]
    pub cast_shadows: bool,
    #[property(category = "Rendering")]
    pub receive_shadows: bool,
    #[property(category = "Rendering")]
    pub wireframe_overlay: bool,
}

impl Default for RenderingTerrainProps {
    fn default() -> Self {
        Self {
            meshing_algorithm: MeshingAlgorithm::Greedy,
            enable_lod: true,
            lod_levels: 4,
            enable_collision: true,
            cast_shadows: true,
            receive_shadows: true,
            wireframe_overlay: false,
        }
    }
}

impl RenderingTerrainProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(ix) = obj.get("meshing_algorithm").and_then(|v| v.as_u64()) {
            self.meshing_algorithm = match ix {
                0 => MeshingAlgorithm::Simple,
                1 => MeshingAlgorithm::Greedy,
                2 => MeshingAlgorithm::SurfaceNets,
                3 => MeshingAlgorithm::MarchingCubes,
                _ => self.meshing_algorithm,
            };
        }
        if let Some(v) = obj.get("enable_lod").and_then(|v| v.as_bool()) {
            self.enable_lod = v;
        }
        if let Some(v) = obj.get("lod_levels").and_then(|v| v.as_u64()) {
            self.lod_levels = v;
        }
        if let Some(v) = obj.get("enable_collision").and_then(|v| v.as_bool()) {
            self.enable_collision = v;
        }
        if let Some(v) = obj.get("cast_shadows").and_then(|v| v.as_bool()) {
            self.cast_shadows = v;
        }
        if let Some(v) = obj.get("receive_shadows").and_then(|v| v.as_bool()) {
            self.receive_shadows = v;
        }
        if let Some(v) = obj.get("wireframe_overlay").and_then(|v| v.as_bool()) {
            self.wireframe_overlay = v;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "meshing_algorithm".to_string(),
            Value::from(self.meshing_algorithm as u64),
        );
        out.insert("enable_lod".to_string(), Value::from(self.enable_lod));
        out.insert("lod_levels".to_string(), Value::from(self.lod_levels));
        out.insert(
            "enable_collision".to_string(),
            Value::from(self.enable_collision),
        );
        out.insert("cast_shadows".to_string(), Value::from(self.cast_shadows));
        out.insert(
            "receive_shadows".to_string(),
            Value::from(self.receive_shadows),
        );
        out.insert(
            "wireframe_overlay".to_string(),
            Value::from(self.wireframe_overlay),
        );
    }
}
