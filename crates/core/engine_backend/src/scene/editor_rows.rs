//! Change-time SceneDB rows consumed by the same passes as the Helio demos.
use helio_component::components::LightComponent;
use helio_pass_billboard::BillboardComponent;
use helio_pass_forward_lit::LightComponent as LightRow;
use pulsar_world_registry::GpuMirrored;

use super::{Transform, Visibility, WorldSceneStore};

/// Marks rows owned by this projection, so removing a light also removes
/// its draw data without touching independently authored pass components.
struct EditorLightRows;

pub fn sync_editor_light_rows(store: &mut WorldSceneStore, editor_mode: bool) {
    let world = store.world_mut();
    let stale: Vec<_> = world
        .query::<&EditorLightRows>()
        .filter(|(entity, _)| world.get::<LightComponent>(*entity).is_none())
        .map(|(entity, _)| entity)
        .collect();
    for entity in stale {
        world.remove::<LightRow>(entity);
        world.remove::<BillboardComponent>(entity);
        world.remove::<EditorLightRows>(entity);
    }
    let entities: Vec<_> = world
        .query::<&LightComponent>()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        let light = world.get::<LightComponent>(entity).expect("queried light");
        let transform = world.get::<Transform>(entity).copied();
        let visible = world.get::<Visibility>(entity).is_none_or(|v| v.visible);
        if !light.general.enabled || !visible || transform.is_none() {
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
        // SceneDB lights currently have no shadow-atlas assignment (same
        // contract as v3_demo_common::spawn_light). Never sample slot zero.
        gpu.shadow_index = u32::MAX;
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

    #[test]
    fn light_rows_follow_world_edits_and_are_removed_with_the_source() {
        let mut store = WorldSceneStore::new();
        let entity = store.spawn(Some("light".into()), "light", None).unwrap();
        let mut light = LightComponent::default();
        light.general.enabled = true;
        store.world_mut().insert(entity, light);
        sync_editor_light_rows(&mut store, true);
        assert!(store.world().get::<LightRow>(entity).is_some());
        assert!(store.world().get::<BillboardComponent>(entity).is_some());
        store
            .world_mut()
            .get_mut::<Transform>(entity)
            .unwrap()
            .position = [2.0, 3.0, 4.0];
        sync_editor_light_rows(&mut store, true);
        assert_eq!(
            store
                .world()
                .get::<LightRow>(entity)
                .unwrap()
                .position_range[..3],
            [2.0, 3.0, 4.0]
        );
        assert_eq!(
            store
                .world()
                .get::<BillboardComponent>(entity)
                .unwrap()
                .world_pos[..3],
            [2.0, 3.0, 4.0]
        );
        store
            .world_mut()
            .get_mut::<Visibility>(entity)
            .unwrap()
            .visible = false;
        sync_editor_light_rows(&mut store, true);
        assert!(store.world().get::<LightRow>(entity).is_none());
        assert!(store.world().get::<BillboardComponent>(entity).is_none());
        store
            .world_mut()
            .get_mut::<Visibility>(entity)
            .unwrap()
            .visible = true;
        sync_editor_light_rows(&mut store, false);
        assert!(store.world().get::<LightRow>(entity).is_some());
        assert!(store.world().get::<BillboardComponent>(entity).is_none());
        store.world_mut().remove::<LightComponent>(entity);
        sync_editor_light_rows(&mut store, true);
        assert!(store.world().get::<LightRow>(entity).is_none());
    }
}
