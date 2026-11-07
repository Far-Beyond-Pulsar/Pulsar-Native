use serde_json::Value;

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
}
