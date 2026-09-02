//! Bridge: `WorldSceneStore` as the script object model's host (#639).
//!
//! Implements the two seams `pulsar_script_object_model` defines --
//! [`StableIdResolver`] (the StableId⇄Entity table script references
//! re-resolve against after save/load/reload/undo-redo/reparenting) and
//! [`ComponentInstanceStore`] (the per-instance JSON records that
//! duplicate/non-live component indexes route through) -- directly over
//! the store's own state: its `by_stable_id` map + `StableId` components,
//! and its `RenderProps.component_instances` JSON projection.
//!
//! Deliberately an impl file, not new storage: everything answered here was
//! already true of `WorldSceneStore`; this module just exposes it under the
//! script-facing contracts so gameplay code never touches editor internals.
//!
//! Index resolution rule for instance JSON (matching
//! `pulsar_scene::component_instances_from_props`, the file-format reader):
//! an entry's explicit `"index"` field wins when present, otherwise its
//! array position IS its index. Entries missing a class name are skipped;
//! a missing `"enabled"` flag means enabled.

use pulsar_scenedb::Entity;
use pulsar_script_object_model::{ComponentInstanceStore, InstanceRecord, StableIdResolver};
use serde_json::Value;

use crate::scene::{Name, RenderProps, StableId, WorldSceneStore};

impl StableIdResolver for WorldSceneStore {
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

impl ComponentInstanceStore for WorldSceneStore {
    fn live_component_index(&self, entity: Entity, class_name: &str) -> Option<u32> {
        self.instance_records(entity)
            .into_iter()
            .find(|(_, record)| record.enabled && record.class_name == class_name)
            .map(|(index, _)| index)
    }

    fn instance_record(&self, entity: Entity, index: u32) -> Option<InstanceRecord> {
        self.instance_records(entity)
            .into_iter()
            .find(|(i, _)| *i == index)
            .map(|(_, record)| record)
    }

    fn set_instance_data(&mut self, entity: Entity, index: u32, data: Value) -> bool {
        // `update_render_props` is stable-id keyed and no-ops silently on
        // unknown ids -- resolve our key first so we can report misses.
        let Some(id) = self.stable_id_of(entity).map(str::to_string) else {
            return false;
        };
        let mut wrote = false;
        self.update_render_props(&id, |props| {
            let Some(entries) = props
                .component_instances
                .as_mut()
                .and_then(Value::as_array_mut)
            else {
                return;
            };
            for (position, entry) in entries.iter_mut().enumerate() {
                if entry_resolved_index(entry, position) != index {
                    continue;
                }
                if let Some(slot) = entry.get_mut("data") {
                    *slot = data.clone();
                    wrote = true;
                }
                break;
            }
        });
        wrote
    }
}

impl WorldSceneStore {
    /// One entity's component-instance records from
    /// `RenderProps.component_instances`, as `(resolved index, record)` in
    /// array order. See this module's doc for the index-resolution rule.
    fn instance_records(&self, entity: Entity) -> Vec<(u32, InstanceRecord)> {
        let Some(props) = self.world().get::<RenderProps>(entity) else {
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
}

// ── World-level identity lookups (#654) ─────────────────────────────────────
//
// Script references resolve LAZILY against whatever world they land in
// (#639), and graph reference nodes run in contexts that hold only a plain
// `&World` — VM trampolines mid-event and generated actors inside their
// tick callbacks — where no `WorldSceneStore` (and therefore no
// `StableIdResolver` impl) exists. Both questions below are answerable from
// the world alone because `StableId`/`Name` ARE world components; these
// free functions are the one shared implementation of that lookup for every
// scripting backend.

/// The live entity carrying `stable_id`, if any.
///
/// First match wins. Entities spawned outside [`WorldSceneStore`] (bare
/// gameplay spawns) carry no `StableId` component and are invisible to this
/// lookup — exactly the "cannot be referenced across sessions" rule of
/// `resolution.rs`.
pub fn entity_with_stable_id(world: &pulsar_scenedb::World, stable_id: &str) -> Option<Entity> {
    world
        .query::<&StableId>()
        .find(|(_, id)| id.0 == stable_id)
        .map(|(entity, _)| entity)
}

/// The first live entity whose display `Name` equals `name`.
///
/// First match in archetype iteration order — name collisions are an authoring
/// hazard, not a resolution ambiguity this function pretends to solve;
/// callers needing disambiguation should prefer stable ids (#639 policy).
pub fn first_entity_named(world: &pulsar_scenedb::World, name: &str) -> Option<Entity> {
    world
        .query::<&Name>()
        .find(|(_, n)| n.0 == name)
        .map(|(entity, _)| entity)
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
    use crate::scene::{ObjectSnapshot, RenderProps, Transform, Visibility};
    use serde_json::json;

    fn snap(stable_id: &str) -> ObjectSnapshot {
        ObjectSnapshot {
            stable_id: stable_id.to_string(),
            name: stable_id.to_string(),
            parent: None,
            transform: Transform::default(),
            visibility: Visibility::default(),
            object_type: crate::scene::ObjectType::Empty,
            render_props: RenderProps::default(),
        }
    }

    fn store_with_instances() -> (WorldSceneStore, Entity) {
        let mut store = WorldSceneStore::load_from_snapshots(&[snap("door")]).unwrap();
        let door = store.entity_for("door").unwrap();
        store.update_render_props("door", |props| {
            props.component_instances = Some(json!([
                { "index": 0, "class_name": "TestGizmo", "enabled": true,
                  "data": { "charges": 1 } },
                { "class_name": "TestGizmo", "enabled": true,
                  "data": { "charges": 2 } },
                { "index": 5, "class_name": "Other", "data": {} }
            ]));
        });
        (store, door)
    }

    #[test]
    fn instance_records_honor_explicit_indexes_and_positions() {
        let (store, door) = store_with_instances();

        let records = WorldSceneStore::instance_records(&store, door);
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
        let (store, door) = store_with_instances();
        assert_eq!(
            <WorldSceneStore as ComponentInstanceStore>::live_component_index(
                &store,
                door,
                "TestGizmo"
            ),
            Some(0)
        );
        assert_eq!(
            <WorldSceneStore as ComponentInstanceStore>::live_component_index(
                &store, door, "Other"
            ),
            Some(5)
        );
        assert_eq!(
            <WorldSceneStore as ComponentInstanceStore>::live_component_index(
                &store, door, "Missing"
            ),
            None
        );
    }

    #[test]
    fn set_instance_data_writes_only_the_targeted_record() {
        let (mut store, door) = store_with_instances();

        let wrote = <WorldSceneStore as ComponentInstanceStore>::set_instance_data(
            &mut store,
            door,
            1,
            json!({ "charges": 22 }),
        );
        assert!(wrote);

        let records = WorldSceneStore::instance_records(&store, door);
        assert_eq!(
            records[0].1.data,
            json!({ "charges": 1 }),
            "record 0 untouched"
        );
        assert_eq!(
            records[1].1.data,
            json!({ "charges": 22 }),
            "record 1 replaced"
        );
    }

    #[test]
    fn set_instance_data_on_unknown_entity_is_false() {
        let mut store = WorldSceneStore::new();
        let phantom = pulsar_scenedb::Entity::from_bits((50u64) << 32);
        assert!(
            !<WorldSceneStore as ComponentInstanceStore>::set_instance_data(
                &mut store,
                phantom,
                0,
                json!({})
            )
        );
    }

    #[test]
    fn resolver_round_trips_through_the_store() {
        let store = WorldSceneStore::load_from_snapshots(&[snap("door"), snap("chest")]).unwrap();

        let door = store.entity_for("door").unwrap();
        assert_eq!(
            StableIdResolver::stable_id_for_entity(&store, door),
            Some("door".to_string())
        );
        assert_eq!(
            StableIdResolver::entity_for_stable_id(&store, "door"),
            Some(door)
        );
        assert!(StableIdResolver::is_entity_alive(&store, door));
        assert_eq!(StableIdResolver::entity_for_stable_id(&store, "nope"), None);
    }

    /// #654: the plain-world lookups find hydrated objects by their StableId
    /// and Name components — the exact queries the graph reference nodes
    /// resolve through at runtime.
    #[test]
    fn world_level_identity_lookups_find_hydrated_objects() {
        let mut store = WorldSceneStore::load_from_snapshots(&[
            ObjectSnapshot {
                name: "Front Door".into(),
                ..snap("door")
            },
            snap("chest"),
        ])
        .unwrap();
        let world = store.world();

        let door = entity_with_stable_id(world, "door").expect("by stable id");
        assert_eq!(store.entity_for("door"), Some(door));
        assert_eq!(first_entity_named(world, "Front Door"), Some(door));
        assert_eq!(
            entity_with_stable_id(world, "chest"),
            store.entity_for("chest")
        );
        assert_eq!(first_entity_named(world, "no such object"), None);

        // Despawned targets stop resolving immediately (lazy resolution).
        store.world_mut().despawn(door);
        let world = store.world();
        assert_eq!(first_entity_named(world, "Front Door"), None);
    }
}
