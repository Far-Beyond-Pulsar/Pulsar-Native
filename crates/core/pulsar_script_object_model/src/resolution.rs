//! Save/load identity for script references (#639): `StableId` ⇄ `Entity`.
//!
//! Raw `Entity` bits are only meaningful within ONE live world's lifetime
//! -- slots are recycled with generation bumps and there is no public API
//! to restore an entity at a chosen id on load. The editor's save/load
//! identity is the store's `StableId(String)` component instead. Script
//! references that must mean the same thing next session therefore
//! serialize as [`SerializedComponentRef`] (`stable_id`, NOT entity bits)
//! and re-resolve lazily against whatever world they land in.
//!
//! **Fallback policy:** resolution is EXACT stable-id match or typed
//! failure. A missing id resolves to `ReferenceLost` -- never silently
//! rebound to some other object by name similarity or position. Because
//! resolution happens at access time against the CURRENT table,
//! references survive scene reloads, undo/redo snapshots, and hierarchy
//! reparenting automatically (none of those change an object's stable id);
//! deleting the target is the one unrecoverable case, and it reports as
//! lost rather than misaddressing.
//!
//! Resolution itself only needs three questions answered, so hosts plug in
//! via the narrow [`StableIdResolver`] trait -- implemented for
//! `WorldSceneStore` in `engine_backend` (see `scene::script_ref_bridge`
//! there); tests use trivial fakes.

use pulsar_scenedb::Entity;
use serde::{Deserialize, Serialize};

use crate::refs::ComponentRef;

/// Read-side bridge to a host's StableId⇄Entity table (implemented by
/// `WorldSceneStore`; see the module doc for why this is a trait).
pub trait StableIdResolver {
    /// Current live entity for a stable id, if the object exists.
    fn entity_for_stable_id(&self, stable_id: &str) -> Option<Entity>;

    /// The stable id an entity was spawned under. `None` for entities not
    /// spawned through the host's tracked path (e.g. bare `World::spawn`) --
    /// those cannot be referenced across sessions.
    fn stable_id_for_entity(&self, entity: Entity) -> Option<String>;

    /// Liveness of one entity (generation-checked).
    fn is_entity_alive(&self, entity: Entity) -> bool;
}

/// A component reference in its save/load form -- what graphs and script
/// state persist as (the exact serialized reference format #639 defines:
/// `{stable_id, class_name, component_index}`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SerializedComponentRef {
    /// The target object's save/load identity.
    pub stable_id: String,
    /// Registered component class name.
    pub class_name: String,
    /// Which instance of the class on that object (panel convention).
    pub component_index: u32,
}

/// Why a saved reference could not be brought back to life.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveRefError {
    /// The stable id no longer maps to any object: deleted, or renamed
    /// without migration. Deliberately a LOUD typed state -- never a silent
    /// rebinding to another object (#639 acceptance).
    #[error(
        "reference target '{stable_id}' no longer exists (deleted or renamed since it was saved)"
    )]
    ReferenceLost { stable_id: String },

    /// The entity is alive but was never given a stable id (spawned
    /// outside the tracked path), so no persistent reference can name it.
    #[error("entity {entity} has no stable id (not spawned through the scene store); it cannot be referenced across save/load")]
    Unidentified { entity: Entity },
}

impl SerializedComponentRef {
    /// Re-resolve against a host's CURRENT table. Component presence is
    /// deliberately not checked here -- that stays lazy at access time
    /// (`ComponentMissing` from the accessors) so a component disabled at
    /// edit time doesn't make the whole reference unresolvable.
    pub fn resolve<R: StableIdResolver + ?Sized>(
        &self,
        resolver: &R,
    ) -> Result<ComponentRef, ResolveRefError> {
        let Some(entity) = resolver.entity_for_stable_id(&self.stable_id) else {
            return Err(ResolveRefError::ReferenceLost {
                stable_id: self.stable_id.clone(),
            });
        };
        Ok(ComponentRef {
            entity,
            class_name: self.class_name.clone(),
            component_index: self.component_index,
        })
    }
}

impl ComponentRef {
    /// Freeze this reference into its save/load form. Fails (typed) if the
    /// target is already gone, or if it has no stable id to freeze.
    pub fn to_serialized<R: StableIdResolver + ?Sized>(
        &self,
        resolver: &R,
    ) -> Result<SerializedComponentRef, ResolveRefError> {
        if !resolver.is_entity_alive(self.entity) {
            return Err(ResolveRefError::ReferenceLost {
                stable_id: resolver
                    .stable_id_for_entity(self.entity)
                    .unwrap_or_default(),
            });
        }
        let Some(stable_id) = resolver.stable_id_for_entity(self.entity) else {
            return Err(ResolveRefError::Unidentified {
                entity: self.entity,
            });
        };
        Ok(SerializedComponentRef {
            stable_id,
            class_name: self.class_name.clone(),
            component_index: self.component_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Two-generation fake: the SAME serialized ref resolved against two
    /// different tables must yield different entities -- that IS survival
    /// across save/load.
    #[derive(Default)]
    struct FakeResolver {
        by_id: HashMap<String, Entity>,
        ids: HashMap<u64, String>,
        alive: Vec<u64>,
    }

    impl FakeResolver {
        fn insert(&mut self, stable_id: &str, entity: Entity) {
            self.by_id.insert(stable_id.to_string(), entity);
            self.ids.insert(entity.bits(), stable_id.to_string());
            self.alive.push(entity.bits());
        }
    }

    impl StableIdResolver for FakeResolver {
        fn entity_for_stable_id(&self, stable_id: &str) -> Option<Entity> {
            self.by_id.get(stable_id).copied()
        }

        fn stable_id_for_entity(&self, entity: Entity) -> Option<String> {
            self.ids.get(&entity.bits()).cloned()
        }

        fn is_entity_alive(&self, entity: Entity) -> bool {
            self.alive.contains(&entity.bits())
        }
    }

    fn e(bits: u64) -> Entity {
        Entity::from_bits(bits)
    }

    /// #639 acceptance: save -> load -> the reference still targets the
    /// intended OBJECT (different world, same stable id).
    #[test]
    fn a_serialized_ref_resolves_across_table_generations() {
        let mut session_a = FakeResolver::default();
        session_a.insert("door", e(0x0000_0005_0000_0003));

        let held = ComponentRef {
            entity: e(0x0000_0005_0000_0003),
            class_name: "TestGizmo".into(),
            component_index: 0,
        };
        let saved = held.to_serialized(&session_a).expect("freezes");
        assert_eq!(saved.stable_id, "door");

        // Next session: same objects, entirely different entity bits.
        let mut session_b = FakeResolver::default();
        let door_b = e(0x0000_00AA_0000_0017);
        session_b.insert("door", door_b);

        let resolved = saved.resolve(&session_b).expect("resolves");
        assert_eq!(resolved.entity, door_b);
        assert_eq!(resolved.class_name, "TestGizmo");
        assert_eq!(resolved.component_index, 0);
    }

    /// #639 acceptance: deleting the target reports typed ReferenceLost --
    /// never a silent rebinding to another object.
    #[test]
    fn deleted_target_resolves_to_reference_lost() {
        let mut session = FakeResolver::default();
        session.insert("door", e(3));

        let saved = SerializedComponentRef {
            stable_id: "door".into(),
            class_name: "X".into(),
            component_index: 0,
        };

        // The target is deleted out from under the saved reference.
        session.by_id.remove("door");
        session.alive.retain(|&b| b != 3);

        assert_eq!(
            saved.resolve(&session),
            Err(ResolveRefError::ReferenceLost {
                stable_id: "door".into()
            })
        );
    }

    /// Freezing a ref to an entity without a stable id is a typed error --
    /// such an object cannot be referenced across sessions. (Alive, so not
    /// "lost"; just unidentified.)
    #[test]
    fn freezing_an_unidentified_entity_is_typed() {
        let mut resolver = FakeResolver::default();
        resolver.alive.push(999); // alive, but never given a stable id
        let orphan = ComponentRef {
            entity: e(999),
            class_name: "TestGizmo".into(),
            component_index: 0,
        };
        assert_eq!(
            orphan.to_serialized(&resolver),
            Err(ResolveRefError::Unidentified { entity: e(999) })
        );
    }

    /// The serialized form round-trips through serde (what graphs persist).
    #[test]
    fn serialized_form_round_trips_through_json() {
        let saved = SerializedComponentRef {
            stable_id: "lever_12".into(),
            class_name: "TriggerComponent".into(),
            component_index: 2,
        };
        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "stable_id": "lever_12",
                "class_name": "TriggerComponent",
                "component_index": 2,
            })
        );
        assert_eq!(
            serde_json::from_value::<SerializedComponentRef>(json).unwrap(),
            saved
        );
    }
}
