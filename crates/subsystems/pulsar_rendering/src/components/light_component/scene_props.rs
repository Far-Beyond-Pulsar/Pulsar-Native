use engine_class_derive::register_scene_props_applier;
use pulsar_reflection::ScenePropsProjector;
use serde_json::Value;
use std::collections::HashMap;

use super::LightComponent;

#[register_scene_props_applier]
impl ScenePropsProjector for LightComponent {
    const CLASS_NAME: &'static str = "LightComponent";

    fn apply_scene_props(props: &mut HashMap<String, Value>, component_data: Option<&Value>) {
        for key in [
            "enabled",
            "affects_world",
            "light_type",
            "light_channels",
            "lighting_channel_0",
            "lighting_channel_1",
            "lighting_channel_2",
            "intensity",
            "intensity_units",
            "exposure_compensation",
            "inverse_squared_falloff",
            "indirect_intensity",
            "max_draw_distance",
            "max_distance_fade_range",
            "color",
            "use_temperature",
            "temperature_kelvin",
            "temperature_tint",
            "color_saturation",
            "color_contrast",
            "use_physical_light_color",
            "range",
            "falloff_start",
            "attenuation_exponent",
            "source_radius",
            "source_length",
            "inner_cone_angle",
            "outer_cone_angle",
            "cast_shadows",
            "cast_static_shadows",
            "cast_dynamic_shadows",
            "cast_volumetric_shadow",
            "cast_contact_shadows",
            "shadow_bias",
            "shadow_normal_bias",
            "shadow_slope_bias",
            "shadow_filter_sharpen",
            "shadow_softness",
            "shadow_resolution_scale",
            "contact_shadow_non_shadow_casting_intensity",
            "affects_volumetric_fog",
            "volumetric_scattering_intensity",
            "volumetric_shadow_intensity",
            "fog_inscattering_intensity",
            "contact_shadow_length",
            "light_function_material",
            "light_function_scale",
            "light_function_fade_distance",
            "light_function_disabled_brightness",
            "mobile_quality_level",
            "ray_tracing_inclusion",
            "virtual_shadow_map_enabled",
            "shadow_cache_mode",
            "per_view_visibility_mask",
            "affects_translucency",
            "affects_reflections",
            "affects_global_illumination",
            "specular_scale",
            "diffuse_scale",
        ] {
            props.remove(key);
        }

        let Some(data) = component_data else {
            return;
        };

        let light = LightComponent::from_component_data(data);
        for (k, v) in light.to_scene_props() {
            props.insert(k, v);
        }
    }
}
