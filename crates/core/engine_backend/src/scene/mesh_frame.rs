//! Transient static-mesh frame views derived directly from SceneDB.
//!
//! `StaticMeshComponent`, `Transform`, and `Visibility` are the authority.
//! This module deliberately does not insert a derived row into `World`, keep
//! subscriptions, or retain an entity map. A frame view is created only at
//! the render boundary and is disposable when that boundary returns.

use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
use helio_component::components::StaticMeshComponent;
use pulsar_scenedb::{Entity, World};

use super::{Transform, Visibility};

/// The renderer-facing transform view for one static mesh.
///
/// This is a transient ABI adapter, not a SceneDB component. It must never be
/// inserted into `World`: doing so would create a second, derived world-state
/// authority and would make replacement/removal dependent on a cache.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedMeshFrame {
    pub model: [f32; 16],
    pub normal_mat: [f32; 12],
    pub position: [f32; 3],
    pub bound_radius: f32,
    pub visible: bool,
}

impl ResolvedMeshFrame {
    /// Derive the view from the current SceneDB rows.
    pub fn from_world(world: &World, entity: Entity) -> Option<Self> {
        if world.get::<StaticMeshComponent>(entity).is_none() {
            return None;
        }
        let transform = world.get::<Transform>(entity).copied()?;
        let visibility = world.get::<Visibility>(entity).copied()?;
        let q = Quat::from_euler(
            EulerRot::YXZ,
            transform.rotation[1].to_radians(),
            transform.rotation[0].to_radians(),
            transform.rotation[2].to_radians(),
        );
        let scale = Vec3::from_array(transform.scale);
        let model =
            Mat4::from_scale_rotation_translation(scale, q, Vec3::from_array(transform.position));
        let normal_cols = Mat3::from_mat4(model).inverse().transpose().to_cols_array();
        Some(Self {
            model: model.to_cols_array(),
            normal_mat: [
                normal_cols[0],
                normal_cols[1],
                normal_cols[2],
                0.0,
                normal_cols[3],
                normal_cols[4],
                normal_cols[5],
                0.0,
                normal_cols[6],
                normal_cols[7],
                normal_cols[8],
                0.0,
            ],
            position: transform.position,
            bound_radius: scale.length().max(0.2) * 0.5,
            visible: visibility.visible,
        })
    }

    /// Pack into the renderer-facing AABB.
    pub fn aabb(&self) -> helio::GpuInstanceAabb {
        let [x, y, z] = self.position;
        let r = self.bound_radius;
        helio::GpuInstanceAabb {
            min: [x - r, y - r, z - r],
            _pad0: 0.0,
            max: [x + r, y + r, z + r],
            _pad1: 0.0,
        }
    }
}

/// Compatibility token for callers that still pass a frame maintainer.
///
/// SceneDB change tracking and GPU mirror upload already handle invalidation;
/// no CPU cache or subscription lifecycle is needed here. Keeping this type
/// temporarily avoids coupling this SceneDB ownership migration to the
/// renderer interaction migration.
#[derive(Default)]
pub struct MeshFrameMaintainer;

impl MeshFrameMaintainer {
    pub fn new() -> Self {
        Self
    }

    pub fn reset(&mut self) {}

    /// Deliberately a no-op. Every render view is derived from the current
    /// World by `SceneRenderProjection::from_store`.
    pub fn maintain(&mut self, _world: &mut World) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_mesh(world: &mut World, position: [f32; 3]) -> Entity {
        let entity = world.spawn();
        world.insert(entity, StaticMeshComponent::default());
        world.insert(
            entity,
            Transform {
                position,
                ..Transform::default()
            },
        );
        world.insert(
            entity,
            Visibility {
                visible: true,
                locked: false,
            },
        );
        entity
    }

    #[test]
    fn derives_from_current_world_rows_without_inserting_a_projection() {
        let mut world = World::new();
        let entity = spawn_mesh(&mut world, [1.0, 2.0, 3.0]);
        let frame = ResolvedMeshFrame::from_world(&world, entity).expect("complete mesh");
        assert_eq!(frame.position, [1.0, 2.0, 3.0]);
        assert!(world.query::<&ResolvedMeshFrame>().next().is_none());
    }

    #[test]
    fn replacement_and_removal_are_seen_immediately_from_world() {
        let mut world = World::new();
        let entity = spawn_mesh(&mut world, [1.0, 2.0, 3.0]);
        assert_eq!(
            ResolvedMeshFrame::from_world(&world, entity)
                .expect("initial frame")
                .position,
            [1.0, 2.0, 3.0]
        );

        world
            .get_mut::<Transform>(entity)
            .expect("transform")
            .position = [7.0, 8.0, 9.0];
        assert_eq!(
            ResolvedMeshFrame::from_world(&world, entity)
                .expect("replacement frame")
                .position,
            [7.0, 8.0, 9.0]
        );

        world.remove::<StaticMeshComponent>(entity);
        assert!(ResolvedMeshFrame::from_world(&world, entity).is_none());
    }
}
