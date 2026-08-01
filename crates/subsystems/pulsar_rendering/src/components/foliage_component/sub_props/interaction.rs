use engine_class_derive::engine_class;
use serde_json::Value;
use std::collections::HashMap;

#[engine_class(no_register, clone, debug, serialize, deserialize)]
#[category("Interaction", category_color = "#A78BFA", default_collapsed = true)]
pub struct InteractionFoliageProps {
    /// Whether this foliage responds to [`helio::FoliageInteractor`]s.
    #[property(category = "Interaction")]
    pub receives_interaction: bool,
    /// Whether the owner object pushes grass aside (registers an interactor at
    /// its position).
    #[property(category = "Interaction")]
    pub interactor_enabled: bool,
    /// Influence radius of the owner's interactor in metres.
    #[property(min = 0.0, max = 50.0, step = 0.1, category = "Interaction")]
    pub interactor_radius: f32,
}

impl Default for InteractionFoliageProps {
    fn default() -> Self {
        Self {
            receives_interaction: true,
            interactor_enabled: true,
            interactor_radius: 1.2,
        }
    }
}

impl InteractionFoliageProps {
    pub(crate) fn apply_from_component_data(&mut self, obj: &serde_json::Map<String, Value>) {
        if let Some(v) = obj.get("receives_interaction").and_then(|v| v.as_bool()) {
            self.receives_interaction = v;
        }
        if let Some(v) = obj.get("interactor_enabled").and_then(|v| v.as_bool()) {
            self.interactor_enabled = v;
        }
        if let Some(v) = obj.get("interactor_radius").and_then(|v| v.as_f64()) {
            self.interactor_radius = v as f32;
        }
    }

    pub(crate) fn apply_to_scene_props(&self, out: &mut HashMap<String, Value>) {
        out.insert(
            "receives_interaction".to_string(),
            Value::from(self.receives_interaction),
        );
        out.insert(
            "interactor_enabled".to_string(),
            Value::from(self.interactor_enabled),
        );
        out.insert(
            "interactor_radius".to_string(),
            Value::from(self.interactor_radius),
        );
    }
}
