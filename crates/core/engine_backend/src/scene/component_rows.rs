//! SceneDB rows authored by components' runtime behavior.
//!
//! Lights and meshes have their own projections (`editor_rows`,
//! `helio_bridge`). Every other render-facing component -- the atmosphere,
//! post-process volumes, fog, reflection captures, water volumes, portals,
//! foliage, camera post-process -- derives its pass rows in
//! `ComponentRuntimeBehavior::sync_component` from its typed value and its
//! owner's transform, queuing the writes in [`PendingWorldWrites`]. This
//! module runs those behaviors for the entities whose components or
//! transform changed and applies the queued writes, so the rows follow the
//! scene in the editor and at runtime alike.
use std::collections::{HashMap, HashSet};
use std::path::Path;

use helio_component::subsystems::PendingWorldWrites;
use pulsar_reflection::{ComponentRuntimeContext, LiveKeySet, RuntimeComponentOwner, Subsystems};
use pulsar_scenedb::{ComponentId, Entity, World};

use super::{RenderProps, StableId, Transform};

struct RowContext<'a> {
    subsystems: Subsystems,
    project_root: &'a Path,
    errors: Vec<String>,
}

impl ComponentRuntimeContext for RowContext<'_> {
    fn subsystems_mut(&mut self) -> &mut Subsystems {
        &mut self.subsystems
    }

    fn project_root(&self) -> &Path {
        self.project_root
    }

    fn report_error(&mut self, message: String) {
        tracing::error!("{message}");
        self.errors.push(message);
    }
}

/// Component ids whose change re-derives an entity's rows: every registered
/// world component class, and the owner's transform.
pub fn component_row_sources() -> Vec<ComponentId> {
    let mut ids: Vec<ComponentId> = pulsar_world_registry::registered_world_component_classes()
        .filter_map(pulsar_world_registry::component_id_for_class)
        .collect();
    ids.push(pulsar_scenedb::component_id::<Transform>());
    ids
}

/// Subscribe `entity` to every row source, so components added, edited or
/// removed later (and transform edits) report it as changed.
pub fn arm_component_row_subscriptions(world: &mut World, entity: Entity) {
    for id in component_row_sources() {
        let _ = world.subscribe_id(entity, id);
    }
}

/// Run the runtime behavior of every registered component on `dirty`
/// entities (`None`: every scene object) and apply the rows they author.
/// Returns the errors components reported.
pub fn sync_component_rows(world: &mut World, dirty: Option<&HashSet<Entity>>, project_root: &Path) -> Vec<String> {
    let entities: Vec<Entity> = match dirty {
        Some(dirty) => dirty.iter().copied().filter(|&entity| world.is_alive(entity)).collect(),
        None => world.query::<&StableId>().map(|(entity, _)| entity).collect(),
    };
    let classes: Vec<&'static str> = pulsar_world_registry::registered_world_component_classes().collect();
    let empty_props = HashMap::new();
    let mut writes = PendingWorldWrites::new();
    let mut live = LiveKeySet::new();
    let mut errors = Vec::new();
    for entity in entities {
        let present: Vec<&str> = classes
            .iter()
            .copied()
            .filter(|class| pulsar_world_registry::world_component_present_for_class(class, world, entity))
            .collect();
        if present.is_empty() {
            continue;
        }
        let transform = world.get::<Transform>(entity).copied().unwrap_or_default();
        let id = world.get::<StableId>(entity).map_or("", StableId::as_str);
        let owner = RuntimeComponentOwner {
            scene_object_id: id,
            position: transform.position,
            rotation: transform.rotation,
            scale: transform.scale,
            props: world.get::<RenderProps>(entity).map_or(&empty_props, |props| &props.props),
        };
        let mut context = RowContext { subsystems: Subsystems::new(), project_root, errors: Vec::new() };
        context.subsystems.register_ref::<PendingWorldWrites>(&mut writes);
        context.subsystems.register_ref::<LiveKeySet>(&mut live);
        context.subsystems.register::<Entity>(entity);
        for (index, class) in present.into_iter().enumerate() {
            pulsar_world_registry::dispatch_world_component_for_class(class, world, entity, &owner, index, &mut context);
        }
        errors.append(&mut context.errors);
    }
    writes.drain_and_apply(world);
    errors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{SceneWorldExt, SpawnObject};

    fn atmosphere(world: &World, entity: Entity) -> Option<helio_pass_sky::AtmosphereComponent> {
        world.get::<helio_pass_sky::AtmosphereComponent>(entity).copied()
    }

    #[test]
    fn component_rows_follow_their_component_and_owner() {
        let mut world = World::new();
        let entity = world.spawn_object(SpawnObject::new("planet").with_id("planet")).unwrap();
        let mut air = helio_component::AtmosphereComponent::default();
        air.placement = helio_component::AtmospherePlacement::PlanetAtOwner;
        world.insert(entity, air);
        world.get_mut::<Transform>(entity).unwrap().position = [1.0, 2.0, 3.0];
        assert!(sync_component_rows(&mut world, None, Path::new(".")).is_empty());
        let row = atmosphere(&world, entity).expect("the atmosphere's row");
        assert_eq!(row.center, [1.0, 2.0, 3.0]);
        assert_eq!(row.placement, helio_pass_sky::atmosphere::placement::CENTER);

        // Moving the owner moves the planet.
        world.get_mut::<Transform>(entity).unwrap().position = [4.0, 5.0, 6.0];
        sync_component_rows(&mut world, Some(&HashSet::from([entity])), Path::new("."));
        assert_eq!(atmosphere(&world, entity).unwrap().center, [4.0, 5.0, 6.0]);

        // Disabling it removes the row.
        world.get_mut::<helio_component::AtmosphereComponent>(entity).unwrap().enabled = false;
        sync_component_rows(&mut world, Some(&HashSet::from([entity])), Path::new("."));
        assert!(atmosphere(&world, entity).is_none());
    }

    #[test]
    fn subscriptions_report_component_and_transform_edits() {
        let mut world = World::new();
        let entity = world.spawn_object(SpawnObject::new("air").with_id("air")).unwrap();
        arm_component_row_subscriptions(&mut world, entity);
        world.insert(entity, helio_component::AtmosphereComponent::default());
        world.get_mut::<Transform>(entity).unwrap().position = [1.0, 0.0, 0.0];
        let events = world.take_component_change_events();
        let sources = component_row_sources();
        assert!(events.iter().any(|e| e.entity == entity && e.component == pulsar_scenedb::component_id::<helio_component::AtmosphereComponent>()));
        assert!(events.iter().all(|e| sources.contains(&e.component)));
    }
}
