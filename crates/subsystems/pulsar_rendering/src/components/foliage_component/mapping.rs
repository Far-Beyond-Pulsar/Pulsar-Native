use serde_json::Value;
use std::collections::HashMap;

use super::FoliageComponent;

impl FoliageComponent {
    pub fn from_component_data(data: &Value) -> Self {
        let mut foliage = Self::default();
        if let Some(obj) = data.as_object() {
            foliage.general.apply_from_component_data(obj);
            foliage.placement.apply_from_component_data(obj);
            foliage.wind.apply_from_component_data(obj);
            foliage.interaction.apply_from_component_data(obj);
            foliage.rendering.apply_from_component_data(obj);
        }
        foliage
    }

    pub fn to_scene_props(&self) -> HashMap<String, Value> {
        let mut out = HashMap::new();
        self.general.apply_to_scene_props(&mut out);
        self.placement.apply_to_scene_props(&mut out);
        self.wind.apply_to_scene_props(&mut out);
        self.interaction.apply_to_scene_props(&mut out);
        self.rendering.apply_to_scene_props(&mut out);
        out
    }
}
