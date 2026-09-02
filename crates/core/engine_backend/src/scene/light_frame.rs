//! SceneDB-resident resolved light frames (Pulsar-Native#636).
//!
//! [`LightComponentGpuMirror`] -- the `#[gpu]`-mirrored companion every
//! enabled light carries -- intentionally holds a *zeroed* position
//! placeholder: `GpuLight` bakes in world-space position, which lives on a
//! separate `Transform` component produced by an independent system on an
//! independent schedule, so no mirror-build-time mapping can pre-combine
//! them (see `LightComponentGpuMirror::to_helio_gpu_light`'s doc,
//! helio-component). Through Pulsar-Native#561 that combination happened
//! once per frame, CPU-side, over a whole-world `(mirror, Transform)`
//! query inside `HelioRenderer::rebuild_light_frame`.
//!
//! This module finishes the residency work: the combination moves to
//! **change time**. A [`ResolvedLightFrame`] component carries the fully
//! combined `GpuLight`; [`LightFrameMaintainer`] keeps those rows current
//! by subscribing (SceneDB#47) to `(entity, LightComponentGpuMirror)` and
//! `(entity, Transform)` for every mirrored light and re-resolving only
//! entities whose components actually changed. Editor gizmo edits and
//! scripted/runtime writes both go through `World`'s `Mut` guard hooks, so
//! both propagate to the rendered frame within the same sync pass --
//! including writes that bypass `WorldSceneStore`'s own dirty-flag
//! bookkeeping entirely.
//!
//! Absence semantics are unchanged from the per-frame query this replaces:
//! a light renders if and only if it has *both* a mirror row (i.e. it is
//! enabled) and a `Transform`. A disabled/removed light's mirror row
//! vanishes, its tracked entry is pruned, and its [`ResolvedLightFrame`] is
//! dropped with it.

use std::collections::HashMap;

use helio::GpuLight;
use helio_component::components::LightComponentGpuMirror;
use pulsar_scenedb::{component_id, ComponentChangeEvent, Entity, SubscriptionId, World};

use super::Transform;

/// Fully-resolved per-light GPU state: the entity's
/// [`LightComponentGpuMirror`] translated via `to_helio_gpu_light()` with
/// the live `Transform`'s world-space position folded into
/// `position_range[0..3]` -- exactly the combination
/// `HelioRenderer::rebuild_light_frame` used to redo for every light on
/// every frame, now maintained incrementally at change time instead.
///
/// Written only by [`LightFrameMaintainer`]; treated as read-only derived
/// state everywhere else. Presence of this row IS "this light is in the
/// render list this pass".
#[derive(Clone, Copy, Debug)]
pub struct ResolvedLightFrame {
    pub light: GpuLight,
}

/// The two subscriptions one tracked light needs: mirror-row changes
/// rebuild the whole `GpuLight`, transform changes re-fold just the
/// position.
#[derive(Clone, Copy)]
struct LightSubscriptions {
    mirror: SubscriptionId,
    transform: SubscriptionId,
}

/// Incrementally maintains one [`ResolvedLightFrame`] row per rendered
/// light, driven by World-level change subscriptions rather than per-frame
/// recomputation (Pulsar-Native#636).
///
/// One maintainer per live `World`: subscription ids are only meaningful
/// against the `World` they were armed on. [`Self::reset`] drops all
/// tracking -- call it whenever the underlying store is swapped wholesale
/// (undo/redo), the same condition `HelioRenderer::force_full_resync`
/// already exists for; the next `maintain` pass re-arms from scratch.
pub struct LightFrameMaintainer {
    /// Live lights (entities with a mirror row) -> their two armed
    /// subscriptions. Key set == exactly what the old per-frame
    /// `(mirror, Transform)` query would have rendered.
    tracked: HashMap<Entity, LightSubscriptions>,
}

impl LightFrameMaintainer {
    pub fn new() -> Self {
        Self {
            tracked: HashMap::new(),
        }
    }

    /// Drop all tracking. The next [`Self::maintain`] re-arms every light
    /// and rebuilds every [`ResolvedLightFrame`] from scratch.
    pub fn reset(&mut self) {
        self.tracked.clear();
    }

    /// Bring the resolved frames back in step with the world: prune lights
    /// whose mirror row vanished, arm + seed new ones, then re-resolve
    /// every tracked light whose subscribed component changed since the
    /// last call. Call once per sync pass, immediately before reading the
    /// rows; requires `&mut World` because arming subscriptions and writing
    /// [`ResolvedLightFrame`] rows are mutations.
    pub fn maintain(&mut self, world: &mut World) {
        self.prune_gone_lights(world);
        self.arm_new_lights(world);
        self.refresh_changed(world);
    }

    /// Entities currently tracked (= currently rendered as lights).
    #[cfg(test)]
    fn tracked(&self) -> &HashMap<Entity, LightSubscriptions> {
        &self.tracked
    }

    /// Stop tracking + unresolve every entity whose mirror row disappeared
    /// (light disabled, component removed, or entity despawned). The mirror
    /// query is over the handful of light entities a scene actually has, so
    /// running it every pass costs nothing worth avoiding.
    fn prune_gone_lights(&mut self, world: &mut World) {
        let mut live = Vec::new();
        for (entity, _mirror) in world.query::<&LightComponentGpuMirror>() {
            live.push(entity);
        }
        let gone: Vec<Entity> = self
            .tracked
            .keys()
            .copied()
            .filter(|e| !live.contains(e))
            .collect();
        for entity in gone {
            let subs = self.tracked.remove(&entity).expect("key came from tracked");
            world.unsubscribe(subs.mirror);
            world.unsubscribe(subs.transform);
            // A despawned entity took its components with it; removing off
            // a dead entity would be a silent no-op, so don't bother.
            if world.is_alive(entity) {
                world.remove::<ResolvedLightFrame>(entity);
            }
        }
    }

    /// Arm subscriptions + seed a resolved frame for every mirrored light
    /// not yet tracked. Seeding goes through the same resolve path as an
    /// event-driven refresh so first appearance and later updates can't
    /// diverge.
    fn arm_new_lights(&mut self, world: &mut World) {
        // Collect first, mutate second -- can't hold query borrows across
        // `world.insert`/`subscribe` calls.
        let untracked: Vec<Entity> = world
            .query::<&LightComponentGpuMirror>()
            .map(|(entity, _)| entity)
            .filter(|entity| !self.tracked.contains_key(entity))
            .collect();

        let mirror_cid = component_id::<LightComponentGpuMirror>();
        let transform_cid = component_id::<Transform>();
        for entity in untracked {
            // Subscriptions key off entity liveness only, not component
            // presence -- arming the transform sub before its `Transform`
            // row exists is deliberate: when the row appears later, its
            // `Inserted` event fires and the next pass resolves the light,
            // instead of it silently never rendering.
            let Some(mirror_sub) = world.subscribe_id(entity, mirror_cid) else {
                continue;
            };
            let Some(transform_sub) = world.subscribe_id(entity, transform_cid) else {
                world.unsubscribe(mirror_sub);
                continue;
            };
            self.tracked.insert(
                entity,
                LightSubscriptions {
                    mirror: mirror_sub,
                    transform: transform_sub,
                },
            );
            self.resolve(world, entity);
        }
    }

    /// Drain the World's batched change events and re-resolve each tracked
    /// light something touched. At-least-once delivery means one event per
    /// write, not coalesced -- resolving twice is idempotent, so treat the
    /// event stream purely as a dirty mask.
    fn refresh_changed(&mut self, world: &mut World) {
        let mirror_cid = component_id::<LightComponentGpuMirror>();
        let transform_cid = component_id::<Transform>();
        let touched: Vec<Entity> = world
            .take_component_change_events()
            .into_iter()
            .filter(|event: &ComponentChangeEvent| {
                event.component == mirror_cid || event.component == transform_cid
            })
            .map(|event| event.entity)
            .collect();
        for entity in touched {
            if self.tracked.contains_key(&entity) {
                self.resolve(world, entity);
            }
        }
    }

    /// Rebuild one light's [`ResolvedLightFrame`] from its current mirror +
    /// transform rows. Missing either row leaves no resolved frame -- the
    /// exact absence semantics the replaced per-frame join had.
    fn resolve(&mut self, world: &mut World, entity: Entity) {
        let mirror = world.get::<LightComponentGpuMirror>(entity).map(|m| *m);
        let transform = world.get::<Transform>(entity).copied();
        match (mirror, transform) {
            (Some(mirror), Some(transform)) => {
                let mut light = mirror.to_helio_gpu_light();
                light.position_range[0] = transform.position[0];
                light.position_range[1] = transform.position[1];
                light.position_range[2] = transform.position[2];
                world.insert(entity, ResolvedLightFrame { light });
            }
            _ => {
                world.remove::<ResolvedLightFrame>(entity);
            }
        }
    }
}

impl Default for LightFrameMaintainer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helio_component::components::LightComponent;
    use pulsar_world_registry::GpuMirrored;

    /// Spawn a light the way the real hydrate path does: insert the
    /// component, then mirror it (enabled lights only).
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
            .map(GpuMirrored::to_gpu_mirror);
        world.insert(entity, mirror.expect("just inserted"));
        entity
    }

    #[test]
    fn maintain_seeds_resolved_frames_with_the_folded_position() {
        let mut world = World::new();
        let mut maintainer = LightFrameMaintainer::new();
        let light = spawn_enabled_light(&mut world, [7.0, 8.0, 9.0]);

        maintainer.maintain(&mut world);

        let frame = world.get::<ResolvedLightFrame>(light).expect("seeded");
        assert_eq!(frame.light.position_range[0..3], [7.0, 8.0, 9.0]);
        assert!(maintainer.tracked().contains_key(&light));
    }

    #[test]
    fn transform_mutation_propagates_on_the_next_pass() {
        // #636: a gizmo or script moving a light must reach the rendered
        // frame without any per-frame CPU recombination -- the maintainer
        // picks the new position up from the subscription event alone.
        let mut world = World::new();
        let mut maintainer = LightFrameMaintainer::new();
        let light = spawn_enabled_light(&mut world, [1.0, 2.0, 3.0]);
        maintainer.maintain(&mut world);

        *world.get_mut::<Transform>(light).expect("transform") = Transform {
            position: [4.0, 5.0, 6.0],
            ..Transform::default()
        };
        // No other change of any kind between the two passes.
        maintainer.maintain(&mut world);

        let frame = world
            .get::<ResolvedLightFrame>(light)
            .expect("still resolved");
        assert_eq!(frame.light.position_range[0..3], [4.0, 5.0, 6.0]);
    }

    #[test]
    fn mirror_refresh_propagates_on_the_next_pass() {
        let mut world = World::new();
        let mut maintainer = LightFrameMaintainer::new();
        let light = spawn_enabled_light(&mut world, [0.0; 3]);
        maintainer.maintain(&mut world);

        // The properties panel's live-edit path: mutate the component, then
        // let its registered refresh hook rebuild the mirror row.
        world
            .get_mut::<LightComponent>(light)
            .unwrap()
            .attenuation
            .range = 25.0;
        pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
            "LightComponent",
            &mut world,
            light,
        );
        maintainer.maintain(&mut world);

        let frame = world
            .get::<ResolvedLightFrame>(light)
            .expect("still resolved");
        assert_eq!(
            frame.light.position_range[3], 25.0,
            "range must follow the refreshed mirror"
        );
    }

    #[test]
    fn disabling_a_light_prunes_its_resolved_frame() {
        let mut world = World::new();
        let mut maintainer = LightFrameMaintainer::new();
        let light = spawn_enabled_light(&mut world, [0.0; 3]);
        maintainer.maintain(&mut world);
        assert!(world.get::<ResolvedLightFrame>(light).is_some());

        // Live-disable: refresh removes the mirror row (the hydrate path's
        // own "disabled means absent" rule), and the next pass must drop
        // both the tracking and the resolved frame.
        world
            .get_mut::<LightComponent>(light)
            .unwrap()
            .general
            .enabled = false;
        pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
            "LightComponent",
            &mut world,
            light,
        );
        maintainer.maintain(&mut world);

        assert!(
            world.get::<ResolvedLightFrame>(light).is_none(),
            "a disabled light must leave the render list"
        );
        assert!(!maintainer.tracked().contains_key(&light));
    }

    #[test]
    fn despawned_lights_are_pruned_without_touching_dead_entities() {
        let mut world = World::new();
        let mut maintainer = LightFrameMaintainer::new();
        let light = spawn_enabled_light(&mut world, [0.0; 3]);
        maintainer.maintain(&mut world);

        world.despawn(light);
        maintainer.maintain(&mut world);

        assert!(!maintainer.tracked().contains_key(&light));
        assert!(!world.is_alive(light));
    }

    #[test]
    fn reset_rearms_and_reseeds_everything() {
        // Wholesale store swap (undo/redo): old subscription ids are dead.
        // Simulated here by resetting mid-life -- the next pass must behave
        // like a fresh maintainer seeing an established scene.
        let mut world = World::new();
        let mut maintainer = LightFrameMaintainer::new();
        spawn_enabled_light(&mut world, [3.0; 3]);
        maintainer.maintain(&mut world);

        maintainer.reset();
        maintainer.maintain(&mut world);

        assert_eq!(maintainer.tracked().len(), 1);
        let entity = *maintainer.tracked().keys().next().unwrap();
        let frame = world.get::<ResolvedLightFrame>(entity).expect("reseeded");
        assert_eq!(frame.light.position_range[0..3], [3.0, 3.0, 3.0]);
    }

    #[test]
    fn a_light_without_a_transform_stays_unresolved_until_one_appears() {
        // Preserves the replaced per-frame join's exact semantics: mirror
        // without Transform rendered nothing. The transform sub is armed
        // anyway, so the light starts rendering the pass after a Transform
        // row shows up.
        let mut world = World::new();
        let mut maintainer = LightFrameMaintainer::new();

        let entity = world.spawn();
        let mut light_component = LightComponent::default();
        light_component.general.enabled = true;
        world.insert(entity, light_component);
        let mirror = world
            .get::<LightComponent>(entity)
            .map(GpuMirrored::to_gpu_mirror);
        world.insert(entity, mirror.expect("just inserted"));

        maintainer.maintain(&mut world);
        assert!(
            world.get::<ResolvedLightFrame>(entity).is_none(),
            "no transform, no frame"
        );

        world.insert(
            entity,
            Transform {
                position: [9.0; 3],
                ..Transform::default()
            },
        );
        maintainer.maintain(&mut world);

        let frame = world
            .get::<ResolvedLightFrame>(entity)
            .expect("resolved once transformed");
        assert_eq!(frame.light.position_range[0..3], [9.0, 9.0, 9.0]);
    }
}
