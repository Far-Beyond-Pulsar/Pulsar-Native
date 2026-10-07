//! Change-time SceneDB rows consumed by the same passes as the Helio demos.
use helio_component::components::LightComponent;
use helio_pass_billboard::BillboardComponent;
use helio_pass_forward_lit::LightComponent as LightRow;
use pulsar_world_registry::GpuMirrored;
use std::collections::HashSet;

use super::{Transform, Visibility};
use pulsar_scene_model::attachments;

/// Marks rows owned by this projection, so removing a light also removes
/// its draw data without touching independently authored pass components.
struct EditorLightRows;

pub fn sync_editor_light_rows(
    world: &mut pulsar_scenedb::World,
    editor_mode: bool,
    dirty: Option<&HashSet<pulsar_scenedb::Entity>>,
) {
    if dirty.is_none() {
        let stale: Vec<_> = world
            .query::<&EditorLightRows>()
            .filter(|(entity, _)| world.get::<LightComponent>(*entity).is_none())
            .map(|(entity, _)| entity)
            .collect();
        for entity in stale {
            world.remove::<LightRow>(entity);
            world.remove::<BillboardComponent>(entity);
            world.remove::<EditorLightRows>(entity);
            super::helio_bridge::project_movability(world, entity);
        }
    }
    let entities: Vec<_> = dirty.map_or_else(
        || {
            world
                .query::<&LightComponent>()
                .map(|(entity, _)| entity)
                .collect()
        },
        |dirty| dirty.iter().copied().collect(),
    );
    for entity in entities {
        if world.get::<LightComponent>(entity).is_none() {
            world.remove::<LightRow>(entity);
            world.remove::<BillboardComponent>(entity);
            if world.get::<EditorLightRows>(entity).is_some() {
                world.remove::<EditorLightRows>(entity);
                super::helio_bridge::project_movability(world, entity);
            }
            continue;
        }
        super::helio_bridge::project_movability(world, entity);
        let light = world.get::<LightComponent>(entity).expect("queried light");
        // A light instance shines from its owner object, while the instance
        // is enabled and the owner visible.
        let transform = attachments::owner_component::<Transform>(world, entity).copied();
        let visible =
            attachments::owner_component::<Visibility>(world, entity).is_none_or(|v| v.visible);
        let enabled = light.general.enabled && attachments::is_enabled(world, entity);
        if !enabled || !visible || transform.is_none() {
            world.remove::<LightRow>(entity);
            world.remove::<BillboardComponent>(entity);
            continue;
        }
        let transform = transform.unwrap();
        let mut gpu = light.to_gpu_mirror().to_helio_gpu_light();
        gpu.position_range[..3].copy_from_slice(&transform.position);
        let rotation = glam::Quat::from_euler(
            glam::EulerRot::YXZ,
            transform.rotation[1].to_radians(),
            transform.rotation[0].to_radians(),
            transform.rotation[2].to_radians(),
        );
        gpu.direction_outer[..3].copy_from_slice(&(rotation * -glam::Vec3::Y).to_array());
        // `shadow_index` carries `cast_shadows` as a request (0 = wants a
        // shadow map, u32::MAX = off). ShadowMatrixPass turns requests into
        // atlas slots on the GPU when the light rows change (Helio#246).
        let billboard = BillboardComponent {
            world_pos: [
                transform.position[0],
                transform.position[1],
                transform.position[2],
                0.0,
            ],
            scale_flags: [0.04, 0.04, 1.0, 0.0],
            color: [
                gpu.color_intensity[0],
                gpu.color_intensity[1],
                gpu.color_intensity[2],
                1.0,
            ],
        };
        let row = LightRow::from(gpu);
        if world.get::<LightRow>(entity) != Some(&row) {
            world.insert(entity, row);
        }
        if editor_mode {
            if world.get::<BillboardComponent>(entity) != Some(&billboard) {
                world.insert(entity, billboard);
            }
        } else {
            world.remove::<BillboardComponent>(entity);
        }
        if world.get::<EditorLightRows>(entity).is_none() {
            world.insert(entity, EditorLightRows);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SceneWorldExt;

    /// An object with one light component instance.
    fn light_object(
        world: &mut pulsar_scenedb::World,
        light: LightComponent,
    ) -> (pulsar_scenedb::Entity, pulsar_scenedb::Entity) {
        let object = world
            .spawn_object(crate::scene::SpawnObject::new("light").with_id("light"))
            .unwrap();
        let instance = attachments::spawn_instance(
            world,
            object,
            pulsar_scene_model::NewInstance::new("LightComponent"),
        )
        .unwrap();
        world.insert(instance, light);
        (object, instance)
    }

    #[test]
    fn light_rows_follow_world_edits_and_are_removed_with_the_source() {
        let mut world = pulsar_scenedb::World::new();
        let mut light = LightComponent::default();
        light.general.enabled = true;
        let (object, instance) = light_object(&mut world, light);
        sync_editor_light_rows(&mut world, true, None);
        assert!(world.get::<LightRow>(instance).is_some());
        assert!(world.get::<BillboardComponent>(instance).is_some());
        world.get_mut::<Transform>(object).unwrap().position = [2.0, 3.0, 4.0];
        sync_editor_light_rows(&mut world, true, None);
        assert_eq!(
            world.get::<LightRow>(instance).unwrap().position_range[..3],
            [2.0, 3.0, 4.0]
        );
        assert_eq!(
            world.get::<BillboardComponent>(instance).unwrap().world_pos[..3],
            [2.0, 3.0, 4.0]
        );
        world.get_mut::<Visibility>(object).unwrap().visible = false;
        sync_editor_light_rows(&mut world, true, None);
        assert!(world.get::<LightRow>(instance).is_none());
        assert!(world.get::<BillboardComponent>(instance).is_none());
        world.get_mut::<Visibility>(object).unwrap().visible = true;
        sync_editor_light_rows(&mut world, false, None);
        assert!(world.get::<LightRow>(instance).is_some());
        assert!(world.get::<BillboardComponent>(instance).is_none());
        // A disabled instance keeps its value but casts no light.
        attachments::set_enabled(&mut world, instance, false);
        sync_editor_light_rows(&mut world, true, None);
        assert!(world.get::<LightRow>(instance).is_none());
        assert!(world.get::<LightComponent>(instance).is_some());
        attachments::detach(&mut world, instance);
        sync_editor_light_rows(&mut world, true, None);
        assert!(world.get::<LightRow>(instance).is_none());
    }

    #[test]
    fn authored_movability_is_projected_into_scenedb() {
        use helio_component::components::ObjectMovability;
        let mut world = pulsar_scenedb::World::new();
        let (_object, instance) = light_object(&mut world, LightComponent::default());
        sync_editor_light_rows(&mut world, true, None);
        assert_eq!(
            world.get::<helio::Movability>(instance),
            Some(&helio::Movability::Static)
        );

        world
            .get_mut::<LightComponent>(instance)
            .unwrap()
            .general
            .movability = ObjectMovability::Movable;
        sync_editor_light_rows(&mut world, true, None);
        assert_eq!(
            world.get::<helio::Movability>(instance),
            Some(&helio::Movability::Movable)
        );

        world.remove::<LightComponent>(instance);
        sync_editor_light_rows(&mut world, true, None);
        assert!(world.get::<helio::Movability>(instance).is_none());
    }
}
