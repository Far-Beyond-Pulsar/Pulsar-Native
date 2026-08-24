//! SceneDB-resident resolved static-mesh instance frames (Pulsar-Native#638).
//!
//! The mirror twin of [`crate::scene::light_frame`] (#636), applied to mesh
//! instances: everything about an instance that is a pure function of its
//! World components -- the model matrix, normal matrix, and bounding sphere
//! from `Transform`; the cull-group choice from `Visibility` -- is computed
//! ONCE at component-change time into a [`ResolvedMeshFrame`] row instead of
//! being re-derived for every entity on every frame inside
//! `rebuild_static_mesh_frame`.
//!
//! Deliberately NOT in the row: anything keyed to GPU-pool state
//! (`mesh_key`, draw counts/offsets come from the var-len pool handles,
//! which legitimately shift when a pool regrows) and the material binding
//! (see `helio_bridge`'s doc for the #638/Helio#231 ownership protocol).
//! The frame builder combines cached frame math with fresh handles, so a
//! pool regrow can never serve a stale key.
//!
//! Absence semantics match the replaced per-frame join exactly: a mesh
//! renders if and only if it has all three of `StaticMeshComponent`,
//! `Transform`, and `Visibility`.

use std::collections::HashMap;

use glam::{EulerRot, Mat3, Mat4, Quat, Vec3};
use helio_component::components::StaticMeshComponent;
use pulsar_scenedb::{component_id, ComponentChangeEvent, Entity, SubscriptionId, World};

use super::{Transform, Visibility};

/// Fully-resolved per-instance CPU frame state for one static mesh --
/// the transform-derived half of `helio::StaticMeshRenderInput`, maintained
/// incrementally from World change subscriptions rather than recomputed
/// wholesale per pass (Pulsar-Native#638).
///
/// Written only by [`MeshFrameMaintainer`]; treated as read-only derived
/// state everywhere else.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedMeshFrame {
    /// Model matrix (scale * rotation-YXZ * translation) as a flat
    /// column-major array, ready for `GpuInstanceData::model`/`prev_model`.
    pub model: [f32; 16],
    /// Inverse-transpose of the model's 3x3, row-major triplets -- the
    /// layout `GpuInstanceData::normal_mat` packs.
    pub normal_mat: [f32; 12],
    /// World-space position (model translation), the AABB center.
    pub position: [f32; 3],
    /// Bounding-sphere radius (`|scale| * 0.5`, clamped like the old inline
    /// derivation), the AABB half-extent.
    pub bound_radius: f32,
    /// Mirrors `Visibility.visible`: false maps to the hidden-cull group.
    pub visible: bool,
}

impl ResolvedMeshFrame {
    /// Pack into the renderer-facing AABB (center +/- radius, w-padded).
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

/// The three subscriptions one tracked mesh needs: any of geometry,
/// transform, or visibility changing invalidates the resolved frame.
#[derive(Clone, Copy)]
struct MeshSubscriptions {
    mesh: SubscriptionId,
    transform: SubscriptionId,
    visibility: SubscriptionId,
}

/// Incrementally maintains one [`ResolvedMeshFrame`] row per static mesh,
/// driven by World-level change subscriptions (Pulsar-Native#638) -- the
/// exact structure [`crate::scene::LightFrameMaintainer`] uses for lights,
/// kept separate because the payload and subscription set differ.
///
/// One maintainer per live `World`; [`Self::reset`] drops all tracking for
/// wholesale store swaps (undo/redo / PIE adoption), after which the next
/// [`Self::maintain`] re-arms everything.
pub struct MeshFrameMaintainer {
    tracked: HashMap<Entity, MeshSubscriptions>,
}

impl MeshFrameMaintainer {
    pub fn new() -> Self {
        Self { tracked: HashMap::new() }
    }

    /// Drop all tracking; the next [`Self::maintain`] re-arms from scratch.
    pub fn reset(&mut self) {
        self.tracked.clear();
    }

    /// Prune gone meshes, arm + seed new ones, refresh changed ones. Call
    /// once per sync pass before reading the rows; needs `&mut World` for
    /// subscriptions and writes.
    pub fn maintain(&mut self, world: &mut World) {
        self.prune_gone_meshes(world);
        self.arm_new_meshes(world);
        self.refresh_changed(world);
    }

    #[cfg(test)]
    fn tracked(&self) -> &HashMap<Entity, MeshSubscriptions> {
        &self.tracked
    }

    fn prune_gone_meshes(&mut self, world: &mut World) {
        let mut live = Vec::new();
        for (entity, _component) in world.query::<&StaticMeshComponent>() {
            live.push(entity);
        }
        let gone: Vec<Entity> =
            self.tracked.keys().copied().filter(|e| !live.contains(e)).collect();
        for entity in gone {
            let subs = self.tracked.remove(&entity).expect("key came from tracked");
            world.unsubscribe(subs.mesh);
            world.unsubscribe(subs.transform);
            world.unsubscribe(subs.visibility);
            if world.is_alive(entity) {
                world.remove::<ResolvedMeshFrame>(entity);
            }
        }
    }

    fn arm_new_meshes(&mut self, world: &mut World) {
        // Collect first, mutate second -- query borrows can't span inserts.
        let untracked: Vec<Entity> = world
            .query::<&StaticMeshComponent>()
            .map(|(entity, _)| entity)
            .filter(|entity| !self.tracked.contains_key(entity))
            .collect();

        let mesh_cid = component_id::<StaticMeshComponent>();
        let transform_cid = component_id::<Transform>();
        let visibility_cid = component_id::<Visibility>();
        for entity in untracked {
            // Arm all three up front regardless of current presence: a late
            // Transform/Visibility row fires its Inserted event and resolves
            // on the next pass, matching the replaced join's semantics
            // (missing row == not rendered until it appears).
            let Some(mesh_sub) = world.subscribe_id(entity, mesh_cid) else { continue };
            let Some(transform_sub) = world.subscribe_id(entity, transform_cid) else {
                world.unsubscribe(mesh_sub);
                continue;
            };
            let Some(visibility_sub) = world.subscribe_id(entity, visibility_cid) else {
                world.unsubscribe(mesh_sub);
                world.unsubscribe(transform_sub);
                continue;
            };
            self.tracked.insert(
                entity,
                MeshSubscriptions { mesh: mesh_sub, transform: transform_sub, visibility: visibility_sub },
            );
            self.resolve(world, entity);
        }
    }

    fn refresh_changed(&mut self, world: &mut World) {
        let mesh_cid = component_id::<StaticMeshComponent>();
        let transform_cid = component_id::<Transform>();
        let visibility_cid = component_id::<Visibility>();
        let touched: Vec<Entity> = world
            .take_component_change_events()
            .into_iter()
            .filter(|e: &ComponentChangeEvent| {
                e.component == mesh_cid || e.component == transform_cid || e.component == visibility_cid
            })
            .map(|e| e.entity)
            .collect();
        for entity in touched {
            if self.tracked.contains_key(&entity) {
                self.resolve(world, entity);
            }
        }
    }

    /// Recompute one mesh's resolved frame from its current component rows;
    /// missing any of the three leaves no resolved row (= not rendered),
    /// preserving the replaced whole-world join's semantics.
    fn resolve(&mut self, world: &mut World, entity: Entity) {
        let mesh_present = world.get::<StaticMeshComponent>(entity).is_some();
        let transform = world.get::<Transform>(entity).copied();
        let visibility = world.get::<Visibility>(entity).copied();

        match (mesh_present, transform, visibility) {
            (true, Some(t), Some(v)) => {
                let q = Quat::from_euler(
                    EulerRot::YXZ,
                    t.rotation[1].to_radians(),
                    t.rotation[0].to_radians(),
                    t.rotation[2].to_radians(),
                );
                let scale = Vec3::from_array(t.scale);
                let model =
                    Mat4::from_scale_rotation_translation(scale, q, Vec3::from_array(t.position));
                let normal_cols = Mat3::from_mat4(model).inverse().transpose().to_cols_array();
                let frame = ResolvedMeshFrame {
                    model: model.to_cols_array(),
                    normal_mat: [
                        normal_cols[0], normal_cols[1], normal_cols[2], 0.0,
                        normal_cols[3], normal_cols[4], normal_cols[5], 0.0,
                        normal_cols[6], normal_cols[7], normal_cols[8], 0.0,
                    ],
                    position: t.position,
                    bound_radius: scale.length().max(0.2) * 0.5,
                    visible: v.visible,
                };
                world.insert(entity, frame);
            }
            _ => {
                world.remove::<ResolvedMeshFrame>(entity);
            }
        }
    }
}

impl Default for MeshFrameMaintainer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_mesh(world: &mut World, position: [f32; 3]) -> Entity {
        let entity = world.spawn();
        world.insert(
            entity,
            Transform { position, ..Transform::default() },
        );
        world.insert(entity, Visibility { visible: true, locked: false });
        world.insert(entity, StaticMeshComponent::default());
        entity
    }

    #[test]
    fn maintain_seeds_resolved_frames_with_derived_math() {
        let mut world = World::new();
        let mut maintainer = MeshFrameMaintainer::new();
        let mesh = spawn_mesh(&mut world, [10.0, 0.0, -4.0]);

        maintainer.maintain(&mut world);

        let frame = world.get::<ResolvedMeshFrame>(mesh).expect("seeded");
        assert_eq!(frame.position, [10.0, 0.0, -4.0]);
        assert!(frame.visible);
        // Column-major flat layout: translation occupies elements 12..15.
        assert_eq!(frame.model[12..15], [10.0, 0.0, -4.0]);
    }

    #[test]
    fn transform_edit_refolds_the_frame_on_the_next_pass() {
        // #638: instance data is derived state updated at CHANGE time, not
        // recomputed wholesale per frame -- moving the object must reach the
        // resolved row through the subscription event alone.
        let mut world = World::new();
        let mut maintainer = MeshFrameMaintainer::new();
        let mesh = spawn_mesh(&mut world, [0.0; 3]);
        maintainer.maintain(&mut world);

        *world.get_mut::<Transform>(mesh).expect("transform") =
            Transform { position: [7.0, 8.0, 9.0], ..Transform::default() };
        maintainer.maintain(&mut world);

        let frame = world.get::<ResolvedMeshFrame>(mesh).expect("still resolved");
        assert_eq!(frame.position, [7.0, 8.0, 9.0]);
    }

    #[test]
    fn visibility_edit_flips_the_cull_flag_on_the_next_pass() {
        let mut world = World::new();
        let mut maintainer = MeshFrameMaintainer::new();
        let mesh = spawn_mesh(&mut world, [0.0; 3]);
        maintainer.maintain(&mut world);

        world
            .get_mut::<Visibility>(mesh)
            .expect("visibility")
            .visible = false;
        maintainer.maintain(&mut world);

        let frame = world.get::<ResolvedMeshFrame>(mesh).expect("still resolved");
        assert!(!frame.visible);
    }

    #[test]
    fn removing_the_component_drops_the_row_and_tracking() {
        let mut world = World::new();
        let mut maintainer = MeshFrameMaintainer::new();
        let mesh = spawn_mesh(&mut world, [0.0; 3]);
        maintainer.maintain(&mut world);
        assert!(world.get::<ResolvedMeshFrame>(mesh).is_some());

        world.remove::<StaticMeshComponent>(mesh);
        maintainer.maintain(&mut world);

        assert!(world.get::<ResolvedMeshFrame>(mesh).is_none());
        assert!(!maintainer.tracked().contains_key(&mesh));
    }

    #[test]
    fn reset_rearms_from_scratch() {
        let mut world = World::new();
        let mut maintainer = MeshFrameMaintainer::new();
        let mesh = spawn_mesh(&mut world, [1.0, 2.0, 3.0]);
        maintainer.maintain(&mut world);

        maintainer.reset();
        maintainer.maintain(&mut world);

        assert_eq!(maintainer.tracked().len(), 1);
        assert!(world.get::<ResolvedMeshFrame>(mesh).is_some());
    }

    #[test]
    fn a_mesh_without_a_transform_stays_unresolved_until_one_appears() {
        // Preserves the replaced join's semantics: component without
        // Transform rendered nothing; the sub armed anyway means the first
        // Transform insert resolves it.
        let mut world = World::new();
        let mut maintainer = MeshFrameMaintainer::new();
        let entity = world.spawn();
        world.insert(entity, StaticMeshComponent::default());
        world.insert(entity, Visibility { visible: true, locked: false });

        maintainer.maintain(&mut world);
        assert!(world.get::<ResolvedMeshFrame>(entity).is_none());

        world.insert(entity, Transform::default());
        maintainer.maintain(&mut world);
        assert!(world.get::<ResolvedMeshFrame>(entity).is_some());
    }
}
