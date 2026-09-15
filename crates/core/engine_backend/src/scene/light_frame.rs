//! Transient light frame views derived directly from SceneDB.
//!
//! `LightComponentGpuMirror` and `Transform` are authoritative. This module
//! intentionally does not insert a combined `GpuLight` row into `World` and
//! does not keep a subscription/hash-map cache between render passes.

use helio::GpuLight;
use helio_component::components::LightComponentGpuMirror;
use pulsar_scenedb::{Entity, World};

use super::Transform;

/// Renderer-facing light data derived from the current SceneDB rows.
///
/// This is a transient ABI adapter, not a World component. SceneDB's GPU
/// mirror owns the component upload; only the world-space transform fold is
/// performed while constructing the current render view.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedLightFrame {
    pub light: GpuLight,
}

impl ResolvedLightFrame {
    pub fn from_world(world: &World, entity: Entity) -> Option<Self> {
        let mirror = world.get::<LightComponentGpuMirror>(entity).copied()?;
        let transform = world.get::<Transform>(entity).copied()?;
        let mut light = mirror.to_helio_gpu_light();
        light.position_range[0] = transform.position[0];
        light.position_range[1] = transform.position[1];
        light.position_range[2] = transform.position[2];
        Some(Self { light })
    }
}

/// Compatibility token for callers that still pass a frame maintainer.
///
/// The World and its GPU mirror are the source of truth. No CPU-side cache is
/// maintained; render views are rebuilt directly from current World rows.
#[derive(Default)]
pub struct LightFrameMaintainer;

impl LightFrameMaintainer {
    pub fn new() -> Self {
        Self
    }

    pub fn reset(&mut self) {}

    pub fn maintain(&mut self, _world: &mut World) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_component::components::LightComponent;
    use pulsar_world_registry::GpuMirrored;

    fn spawn_enabled_light(world: &mut World, position: [f32; 3]) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            Transform {
                position,
                ..Transform::default()
            },
        );
        let mut light = LightComponent::default();
        light.general.enabled = true;
        world.insert(entity, light);
        let mirror = world
            .get::<LightComponent>(entity)
            .map(GpuMirrored::to_gpu_mirror)
            .expect("light mirror");
        world.insert(entity, mirror);
        entity
    }

    #[test]
    fn derives_position_from_current_world_rows_without_inserting_a_projection() {
        let mut world = World::new();
        let entity = spawn_enabled_light(&mut world, [7.0, 8.0, 9.0]);
        let frame = ResolvedLightFrame::from_world(&world, entity).expect("complete light");
        assert_eq!(frame.light.position_range[0..3], [7.0, 8.0, 9.0]);
        assert!(world.query::<&ResolvedLightFrame>().next().is_none());
    }

    #[test]
    fn replacement_and_removal_are_seen_immediately_from_world() {
        let mut world = World::new();
        let entity = spawn_enabled_light(&mut world, [1.0, 2.0, 3.0]);
        assert_eq!(
            ResolvedLightFrame::from_world(&world, entity)
                .expect("initial frame")
                .light
                .position_range[0..3],
            [1.0, 2.0, 3.0]
        );

        world
            .get_mut::<Transform>(entity)
            .expect("transform")
            .position = [4.0, 5.0, 6.0];
        assert_eq!(
            ResolvedLightFrame::from_world(&world, entity)
                .expect("replacement frame")
                .light
                .position_range[0..3],
            [4.0, 5.0, 6.0]
        );

        world.remove::<LightComponentGpuMirror>(entity);
        assert!(ResolvedLightFrame::from_world(&world, entity).is_none());
    }
}
