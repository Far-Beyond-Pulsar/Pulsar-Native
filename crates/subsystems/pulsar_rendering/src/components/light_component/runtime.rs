use engine_class_derive::{register_runtime_behavior, register_world_component};
use helio::{GpuLight, LightType as HelioLightType, Renderer, SceneActor};
use pulsar_reflection::{
    get_subsystem, scene_id_to_tag, ComponentRuntimeBehavior, ComponentRuntimeContext,
    RuntimeComponentOwner,
};

use super::{LightComponent, LightType};

// Phase B5 (Pulsar-Native#556).
#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for LightComponent {
    const CLASS_NAME: &'static str = "LightComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let light = component;
        let tag = scene_id_to_tag(owner.scene_object_id);

        if !light.general.enabled {
            // Disabled — drop the light we previously inserted, if any.
            let scene = get_subsystem!(context, Renderer).scene_mut();
            if let Some(id) = scene.light_by_tag(tag) {
                let _ = scene.remove_light(id);
            }
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
            ..Default::default()
        };

        // Helio owns the scene: ask it whether we already have a light for
        // this object rather than tracking handles editor-side.
        let scene = get_subsystem!(context, Renderer).scene_mut();
        match scene.light_by_tag(tag) {
            Some(id) => {
                let _ = scene.update_light(id, gpu);
            }
            None => {
                scene.insert_actor(SceneActor::light_with_tag(gpu, tag));
            }
        }
    }
}
