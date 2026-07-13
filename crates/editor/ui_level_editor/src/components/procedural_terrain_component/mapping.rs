use serde_json::Value;
use std::collections::HashMap;

use super::ProceduralTerrainComponent;

impl ProceduralTerrainComponent {
    pub fn from_component_data(data: &Value) -> Self {
        let mut terrain = Self::default();
        if let Some(obj) = data.as_object() {
            terrain.general.apply_from_component_data(obj);
            terrain.generation.apply_from_component_data(obj);
            terrain.transform.apply_from_component_data(obj);
            terrain.material.apply_from_component_data(obj);
            terrain.rendering.apply_from_component_data(obj);
        }
        terrain
    }

    pub fn to_scene_props(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        self.general.apply_to_scene_props(&mut out);
        self.generation.apply_to_scene_props(&mut out);
        self.transform.apply_to_scene_props(&mut out);
        self.material.apply_to_scene_props(&mut out);
        self.rendering.apply_to_scene_props(&mut out);
        out
    }
}
