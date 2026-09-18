//! `pulsar_scenedb::World` as the script object model's host (#639).
//!
//! Implements the two seams this crate defines --
//! [`StableIdResolver`] (the StableId⇄Entity mapping script references
//! re-resolve against after save/load/reload/undo-redo/reparenting) and
//! [`ComponentInstanceStore`] (the per-instance JSON records that
//! duplicate/non-live component indexes route through) -- directly over the
//! world's own components: `StableId`, and the `RenderProps.component_instances`
//! JSON projection.
//!
//! Index resolution rule for instance JSON (matching
//! `pulsar_scene::component_instances_from_props`, the file-format reader):
//! an entry's explicit `"index"` field wins when present, otherwise its
//! array position IS its index. Entries missing a class name are skipped;
//! a missing `"enabled"` flag means enabled.

use pulsar_scene_model::{RenderProps, SceneWorldExt};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use crate::{ComponentInstanceStore, InstanceRecord, StableIdResolver};

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

impl ComponentInstanceStore for World {
    fn live_component_index(&self, entity: Entity, class_name: &str) -> Option<u32> {
        instance_records(self, entity)
            .into_iter()
            .find(|(_, record)| record.enabled && record.class_name == class_name)
            .map(|(index, _)| index)
    }

    fn instance_record(&self, entity: Entity, index: u32) -> Option<InstanceRecord> {
        instance_records(self, entity)
            .into_iter()
            .find(|(i, _)| *i == index)
            .map(|(_, record)| record)
    }

    fn set_instance_data(&mut self, entity: Entity, index: u32, data: Value) -> bool {
        let Some(mut props) = self.get_mut::<RenderProps>(entity) else {
            return false;
        };
        let Some(entries) = props
            .component_instances
            .as_mut()
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        for (position, entry) in entries.iter_mut().enumerate() {
            if entry_resolved_index(entry, position) != index {
                continue;
            }
            return match entry.get_mut("data") {
                Some(slot) => {
                    *slot = data;
                    true
                }
                None => false,
            };
        }
        false
    }
}

/// One entity's component-instance records from
/// `RenderProps.component_instances`, as `(resolved index, record)` in array
/// order. See this module's doc for the index-resolution rule.
pub fn instance_records(world: &World, entity: Entity) -> Vec<(u32, InstanceRecord)> {
    let Some(props) = world.get::<RenderProps>(entity) else {
        return Vec::new();
    };
    let Some(array) = props.component_instances.as_ref().and_then(Value::as_array) else {
        return Vec::new();
    };
    array
        .iter()
        .enumerate()
        .filter_map(|(position, entry)| {
            let object = entry.as_object()?;
            let class_name = object.get("class_name")?.as_str()?.to_string();
            let record = InstanceRecord {
                class_name,
                enabled: object
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(true),
                data: object.get("data").cloned().unwrap_or(Value::Null),
            };
            Some((entry_resolved_index(entry, position), record))
        })
        .collect()
}

/// An entry's resolved index: its explicit `"index"` field, else its array
/// position -- the exact rule `pulsar_scene::component_instances_from_props`
/// applies when reading files.
fn entry_resolved_index(entry: &Value, position: usize) -> u32 {
    entry
        .as_object()
        .and_then(|o| o.get("index"))
        .and_then(Value::as_u64)
        .map(|i| i as u32)
        .unwrap_or(position as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_scene_model::SpawnObject;
    use serde_json::json;

    fn world_with_instances() -> (World, Entity) {
        let mut world = World::new();
        let door = world
            .spawn_object(SpawnObject::new("door").with_id("door"))
            .unwrap();
        world.insert(
            door,
            RenderProps {
                component_instances: Some(json!([
                    { "index": 0, "class_name": "TestGizmo", "enabled": true,
                      "data": { "charges": 1 } },
                    { "class_name": "TestGizmo", "enabled": true,
                      "data": { "charges": 2 } },
                    { "index": 5, "class_name": "Other", "data": {} }
                ])),
                ..RenderProps::default()
            },
        );
        (world, door)
    }

    #[test]
    fn instance_records_honor_explicit_indexes_and_positions() {
        let (world, door) = world_with_instances();
        let records = instance_records(&world, door);
        assert_eq!(records.len(), 3);
        // Explicit index wins...
        assert_eq!(records[0].0, 0);
        // ...absence falls back to array POSITION (1 here), not the last
        // explicit index plus one.
        assert_eq!(records[1].0, 1);
        assert_eq!(records[2].0, 5);
        // Missing `enabled` means enabled.
        assert!(records[2].1.enabled);
    }

    #[test]
    fn live_component_index_finds_the_first_enabled_instance() {
        let (world, door) = world_with_instances();
        assert_eq!(world.live_component_index(door, "TestGizmo"), Some(0));
        assert_eq!(world.live_component_index(door, "Other"), Some(5));
        assert_eq!(world.live_component_index(door, "Missing"), None);
    }

    #[test]
    fn set_instance_data_writes_only_the_targeted_record() {
        let (mut world, door) = world_with_instances();
        assert!(world.set_instance_data(door, 1, json!({ "charges": 22 })));

        let records = instance_records(&world, door);
        assert_eq!(records[0].1.data, json!({ "charges": 1 }), "record 0 untouched");
        assert_eq!(records[1].1.data, json!({ "charges": 22 }), "record 1 replaced");
    }

    #[test]
    fn set_instance_data_on_unknown_entity_is_false() {
        let mut world = World::new();
        let phantom = Entity::from_bits((50u64) << 32);
        assert!(!world.set_instance_data(phantom, 0, json!({})));
    }

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
