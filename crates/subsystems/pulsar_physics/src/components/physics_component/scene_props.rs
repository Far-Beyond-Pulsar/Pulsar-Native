use engine_class_derive::register_scene_props_applier;
use pulsar_reflection::ScenePropsProjector;
use serde_json::Value;
use std::collections::HashMap;

use super::PhysicsComponent;

#[register_scene_props_applier]
impl ScenePropsProjector for PhysicsComponent {
    const CLASS_NAME: &'static str = "PhysicsComponent";

    /// Clears the keys this class's values once occupied in an object's
    /// props (the level-file migration); the values live in the component.
    fn apply_scene_props(props: &mut HashMap<String, Value>, _component_data: Option<&Value>) {
        for key in [
            "enabled",
            "collision_enabled",
            "generate_overlap_events",
            "simulation_generates_hits",
            "collision_on_rotation",
            "collision_preset",
            "override_collision_preset",
            "create_physics_state",
            "complex_as_simple",
            "collision_channel",
            "channel_responses",
            "all_channels_response",
            "physics_material",
            "override_physics_material",
            "friction",
            "restitution",
            "combined_friction",
            "combined_restitution",
            "simulate_physics",
            "generate_collision_events",
            "wake_on_collision",
            "enable_sleeping",
            "sleep_threshold",
            "max_delta_time",
            "interface",
            "enable_transform_interpolation",
            "sync_to_physics",
            "interpolation_method",
            "min_translation_for_interpolation",
            "min_rotation_for_interpolation",
            "override_linear_velocity",
            "override_angular_velocity",
        ] {
            props.remove(key);
        }
    }
}
