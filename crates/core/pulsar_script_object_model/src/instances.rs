//! Duplicate-instance storage seam: where the *other* component instances
//! live.
//!
//! `World` stores one typed value per `(entity, ComponentId)`, so of N
//! instances of a class on one entity exactly ONE is live-typed (the
//! properties panel's rule, Pulsar-Native#519). Every other instance exists
//! only as its own serialized record. In the editor those records live in
//! the metadata DB; at runtime they live in `RenderProps`'s JSON projection.
//! Neither home belongs in this crate -- so this module defines the narrow
//! read/write seam accessors route non-live indexes through, and each host
//! (`WorldSceneStore` in `engine_backend`, tests' fake stores) implements it
//! over its own storage.
//!
//! The record shape mirrors the editor's persisted component list exactly:
//! one entry = `{class_name, enabled, data}` at a positional index.

use pulsar_scenedb::Entity;
use serde_json::Value;

/// One serialized component instance record -- exactly one entry of an
/// object's persisted component list.
#[derive(Clone, Debug, PartialEq)]
pub struct InstanceRecord {
    /// Registered class name this instance was created from.
    pub class_name: String,
    /// Enabled instances hydrate; disabled ones keep their data but don't
    /// drive the live-typed value (same semantics as the editor's list).
    pub enabled: bool,
    /// The FULL component JSON for this instance (whole struct, including
    /// `#[sub_props]` nesting -- never a sparse fragment).
    pub data: Value,
}

/// Read/write access to an object's per-instance component records.
///
/// Implementors own the actual storage; this trait only fixes the shape the
/// object model routes through. All methods take the entity explicitly --
/// implementors are NOT expected to validate liveness (the accessors do
/// that first); they just index their storage.
pub trait ComponentInstanceStore {
    /// Positional index of the FIRST ENABLED record whose class matches
    /// `class_name` -- the live-typed one (panel parity,
    /// `SceneDatabase::live_typed_component_index`). `None` when no enabled
    /// instance of the class exists in the list.
    fn live_component_index(&self, entity: Entity, class_name: &str) -> Option<u32>;

    /// One record by position. `None` when out of range.
    fn instance_record(&self, entity: Entity, index: u32) -> Option<InstanceRecord>;

    /// Replace one record's component JSON, keeping its position. Returns
    /// `false` (no-op) when `index` is out of range.
    fn set_instance_data(&mut self, entity: Entity, index: u32, data: Value) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeInstanceStore;

    fn record(class_name: &str, value: i64, enabled: bool) -> InstanceRecord {
        InstanceRecord {
            class_name: class_name.into(),
            enabled,
            data: serde_json::json!({ "charges": value }),
        }
    }

    #[test]
    fn live_component_index_skips_disabled_and_mismatched_records() {
        let e = pulsar_scenedb::Entity::from_bits(7);
        let mut store = FakeInstanceStore::default();
        store.attach(
            e,
            &[
                record("A", 1, false),
                record("B", 2, true),
                record("A", 3, false),
                record("A", 4, true),
            ],
        );

        assert_eq!(store.live_component_index(e, "A"), Some(3));
        assert_eq!(store.live_component_index(e, "B"), Some(1));
        assert_eq!(store.live_component_index(e, "Missing"), None);
    }

    #[test]
    fn set_instance_data_is_positional_and_range_checked() {
        let e = pulsar_scenedb::Entity::from_bits(9);
        let mut store = FakeInstanceStore::default();
        store.attach(e, &[record("A", 1, true)]);

        assert!(store.set_instance_data(e, 0, serde_json::json!({ "charges": 99 })));
        assert!(!store.set_instance_data(e, 5, serde_json::json!({})));
        assert_eq!(
            store.instance_record(e, 0).unwrap().data,
            serde_json::json!({ "charges": 99 })
        );
    }

    #[test]
    fn records_of_other_entities_are_invisible() {
        let mine = pulsar_scenedb::Entity::from_bits(1);
        let theirs = pulsar_scenedb::Entity::from_bits(2);
        let mut store = FakeInstanceStore::default();
        store.attach(mine, &[record("A", 1, true)]);

        assert_eq!(store.instance_record(theirs, 0), None);
        assert_eq!(store.live_component_index(theirs, "A"), None);
        assert!(!store.set_instance_data(theirs, 0, serde_json::json!({})));
    }
}
