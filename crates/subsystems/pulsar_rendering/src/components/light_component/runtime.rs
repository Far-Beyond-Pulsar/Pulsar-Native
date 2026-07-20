use engine_class_derive::register_runtime_behavior;
use helio::{GpuLight, LightType as HelioLightType, Renderer, SceneActor};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner, get_subsystem,
    scene_id_to_tag,
};
use serde_json::Value;

use super::{LightComponent, LightType};

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for LightComponent {
    const CLASS_NAME: &'static str = "LightComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let light = Self::from_component_data(component_data);
        if !light.general.enabled {
            return;
        }

        let helio_type = match light.general.light_type {
            LightType::Directional => HelioLightType::Directional,
            LightType::Point => HelioLightType::Point,
            LightType::Spot => HelioLightType::Spot,
            LightType::Area => HelioLightType::Point, // helio has no Area; nearest equivalent
        };

        let [px, py, pz] = owner.position;

        let gpu = GpuLight {
            position_range: [px, py, pz, light.attenuation.range],
            direction_outer: [
                0.0,
                -1.0,
                0.0,
                light.attenuation.outer_cone_angle.to_radians(),
            ],
            color_intensity: [
                light.color.color[0],
                light.color.color[1],
                light.color.color[2],
                light.intensity.intensity,
            ],
            shadow_index: if light.shadows.cast_shadows {
                0
            } else {
                u32::MAX
            },
            light_type: helio_type as u32,
            inner_angle: light.attenuation.inner_cone_angle.to_radians(),
            _pad: 0,
            god_rays_enabled: 0,
            god_rays_density: 0.0,
            god_rays_weight: 0.0,
            god_rays_decay: 0.0,
            god_rays_exposure: 0.0,
            _pad2: [0; 3],
        };

        let tag = scene_id_to_tag(owner.scene_object_id);
        let renderer = get_subsystem!(context, Renderer);
        renderer
            .scene_mut()
            .insert_actor(SceneActor::light_with_tag(gpu, tag));
    }
}
