use engine_class_derive::register_scene_props_applier;
use pulsar_reflection::ScenePropsProjector;
use serde_json::Value;
use std::collections::HashMap;

use super::RigidbodyComponent;

#[register_scene_props_applier]
impl ScenePropsProjector for RigidbodyComponent {
    const CLASS_NAME: &'static str = "RigidbodyComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        for key in [
            "enabled",
            "mass",
            "mass_scale",
            "density",
            "motion_type",
            "override_mass",
            "linear_velocity",
            "angular_velocity",
            "auto_compute_linear_velocity",
            "auto_compute_angular_velocity",
            "compute_velocity_from_displacement",
            "linear_damping",
            "angular_damping",
            "default_linear_damping",
            "default_angular_damping",
            "disable_animation_damping",
            "disable_pose_animation_damping",
            "gravity_enabled",
            "gravity_scale",
            "custom_gravity",
            "apply_force",
            "apply_force_position",
            "apply_impulse",
            "apply_impulse_position",
            "apply_torque",
            "apply_angular_impulse",
            "disable_all_forces",
            "lock_linear_x",
            "lock_linear_y",
            "lock_linear_z",
            "lock_angular_x",
            "lock_angular_y",
            "lock_angular_z",
            "auto_update_constraints",
            "enable_locked_motions_override",
            "enable_transform_interpolation",
            "interpolation_method",
            "min_translation_for_interpolation",
            "min_rotation_for_interpolation",
            "enable_sync_to_physics",
            "enable_sleeping",
            "sleep_threshold",
            "wake_on_collision",
            "disable_collision",
            "enable_gravity",
        ] {
            props.remove(key);
        }

        let Some(data) = component_data else {
            return;
        };

        let rigidbody = RigidbodyComponent::from_component_data(data);
        for (k, v) in rigidbody.to_scene_props() {
            props.insert(k, v);
        }
    }
}
