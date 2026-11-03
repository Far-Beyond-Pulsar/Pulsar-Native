use engine_class_derive::register_scene_props_applier;
use pulsar_reflection::ScenePropsProjector;
use serde_json::Value;
use std::collections::HashMap;

use super::FoliageComponent;

#[register_scene_props_applier]
impl ScenePropsProjector for FoliageComponent {
    const CLASS_NAME: &'static str = "FoliageComponent";

    /// Clears the keys this class's values once occupied in an object's
    /// props (the level-file migration); the values live in the component.
    fn apply_scene_props(props: &mut HashMap<String, Value>, _component_data: Option<&Value>) {
        for key in [
            "enabled",
            "density",
            "density_layer",
            "height_min",
            "height_max",
            "width_min",
            "width_max",
            "slope_min_degrees",
            "slope_max_degrees",
            "altitude_min",
            "altitude_max",
            "layer_extent",
            "has_infinite_extent",
            "trunk_sway",
            "branch_flutter",
            "leaf_jitter",
            "interaction_stiffness",
            "use_global_wind",
            "wind_enabled",
            "wind_direction",
            "wind_speed",
            "gust_amplitude",
            "gust_frequency",
            "turbulence_scale",
            "receives_interaction",
            "interactor_enabled",
            "interactor_radius",
            "two_sided",
            "casts_shadow",
            "lod_distance_0",
            "lod_distance_1",
            "lod_distance_2",
            "lod_distance_3",
            "base_color",
            "roughness",
            "metallic",
        ] {
            props.remove(key);
        }
    }
}
