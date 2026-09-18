//! World-level identity lookups for script references (#654).
//!
//! Script references resolve LAZILY against whatever world they land in
//! (#639), and graph reference nodes run in contexts that hold only a plain
//! `&World` -- VM trampolines mid-event and generated actors inside their
//! tick callbacks. Both questions below are answerable from the world alone
//! because `StableId`/`Name` ARE world components; these free functions are
//! the one shared implementation of that lookup for every scripting backend.
//! (The `StableIdResolver` / `ComponentInstanceStore` seams themselves are
//! implemented for `World` in `pulsar_script_object_model::world_host`.)

use pulsar_scenedb::Entity;

use crate::scene::{Name, SceneWorldExt};

/// The live entity carrying `stable_id`, if any.
///
/// First match wins. Entities spawned without a `StableId` component (bare
/// gameplay spawns) are invisible to this lookup -- exactly the "cannot be
/// referenced across sessions" rule of `resolution.rs`.
pub fn entity_with_stable_id(world: &pulsar_scenedb::World, stable_id: &str) -> Option<Entity> {
    world.entity_for(stable_id)
}

/// The first live entity whose display `Name` equals `name`.
///
/// First match in archetype iteration order -- name collisions are an authoring
/// hazard, not a resolution ambiguity this function pretends to solve;
/// callers needing disambiguation should prefer stable ids (#639 policy).
pub fn first_entity_named(world: &pulsar_scenedb::World, name: &str) -> Option<Entity> {
    world
        .query::<&Name>()
        .find(|(_, n)| n.0 == name)
        .map(|(entity, _)| entity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::SpawnObject;

    /// #654: the plain-world lookups find spawned objects by their StableId
    /// and Name components -- the exact queries the graph reference nodes
    /// resolve through at runtime.
    #[test]
    fn world_level_identity_lookups_find_spawned_objects() {
        let mut world = pulsar_scenedb::World::new();
        let door = world
            .spawn_object(SpawnObject::new("Front Door").with_id("door"))
            .unwrap();
        let chest = world
            .spawn_object(SpawnObject::new("chest").with_id("chest"))
            .unwrap();

        assert_eq!(entity_with_stable_id(&world, "door"), Some(door));
        assert_eq!(first_entity_named(&world, "Front Door"), Some(door));
        assert_eq!(entity_with_stable_id(&world, "chest"), Some(chest));
        assert_eq!(first_entity_named(&world, "no such object"), None);

        // Despawned targets stop resolving immediately (lazy resolution).
        world.despawn(door);
        assert_eq!(first_entity_named(&world, "Front Door"), None);
    }
}
