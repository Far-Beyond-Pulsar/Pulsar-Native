//! `pulsar_scenedb::World` as the script object model's host (#639).
//!
//! Implements [`StableIdResolver`] (the StableId⇄Entity mapping script
//! references re-resolve against after save/load/reload/undo-redo/
//! reparenting) directly over the world's own `StableId` components.
//! Component instances need no seam: each is its own entity holding its own
//! typed value (Pulsar-Native#1035, D1).

use pulsar_scene_model::SceneWorldExt;
use pulsar_scenedb::{Entity, World};

use crate::StableIdResolver;

impl StableIdResolver for World {
    fn entity_for_stable_id(&self, stable_id: &str) -> Option<Entity> {
        self.entity_for(stable_id)
    }

    fn stable_id_for_entity(&self, entity: Entity) -> Option<String> {
        self.stable_id_of(entity).map(str::to_string)
    }

    fn is_entity_alive(&self, entity: Entity) -> bool {
        self.is_alive(entity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::SpawnObject;

    #[test]
    fn resolver_round_trips_through_the_world() {
        let mut world = World::new();
        let door = world
            .spawn_object(SpawnObject::new("door").with_id("door"))
            .unwrap();
        assert_eq!(world.stable_id_for_entity(door), Some("door".to_string()));
        assert_eq!(world.entity_for_stable_id("door"), Some(door));
        assert!(world.is_entity_alive(door));
        assert_eq!(world.entity_for_stable_id("nope"), None);
    }
}
