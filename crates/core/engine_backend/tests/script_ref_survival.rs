//! #639 integration: script references survive save/load through the REAL
//! `WorldSceneStore` round trip.
//!
//! Registers one small component class (`BridgeGizmo`) into both registries
//! so the full hydrate/edit path runs exactly as it would for a real
//! component, without depending on renderer-side classes' property shapes.

use std::sync::Arc;

use engine_backend::scene::{ObjectSnapshot, RenderProps, Transform, Visibility, WorldSceneStore};
use parking_lot::RwLock;
use pulsar_reflection::{EngineClass, PropertyMetadata, RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY};
use pulsar_scenedb::World;
use pulsar_script_object_model::{
    ComponentRef, ResolveRefError, ScriptRefError, SerializedComponentRef,
};

// ── a minimal registered class, self-contained to this test binary ────────

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct BridgeGizmo {
    charge: i32,
}

impl EngineClass for BridgeGizmo {
    fn class_name() -> &'static str {
        "BridgeGizmo"
    }

    fn get_properties(&self) -> Vec<PropertyMetadata> {
        let type_info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY
            .get::<i32>()
            .expect("i32 prim registered");
        vec![PropertyMetadata {
            name: "charge",
            display_name: "Charge".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info,
            getter: Box::new(|c: &dyn EngineClass| {
                Box::new(c.as_any().downcast_ref::<BridgeGizmo>().unwrap().charge)
            }),
            setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                if let Some(v) = v.downcast_ref::<i32>() {
                    c.as_any_mut().downcast_mut::<BridgeGizmo>().unwrap().charge = *v;
                }
            }),
        }]
    }

    fn create_default() -> Box<dyn EngineClass> {
        Box::new(Self::default())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_boxed(&self) -> Box<dyn EngineClass> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}

fn bridge_gizmo_get(world: &World, entity: pulsar_scenedb::Entity) -> Option<&dyn EngineClass> {
    world
        .get::<BridgeGizmo>(entity)
        .map(|c| c as &dyn EngineClass)
}

fn bridge_gizmo_get_mut(
    world: &mut World,
    entity: pulsar_scenedb::Entity,
) -> Option<&mut dyn EngineClass> {
    world
        .get_mut::<BridgeGizmo>(entity)
        .map(|c| c.into_inner() as &mut dyn EngineClass)
}

fn bridge_gizmo_hydrate(
    world: &mut World,
    entity: pulsar_scenedb::Entity,
    data: &serde_json::Value,
) -> Result<(), String> {
    let parsed: BridgeGizmo = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    world.insert(entity, parsed);
    Ok(())
}

fn bridge_gizmo_remove(world: &mut World, entity: pulsar_scenedb::Entity) {
    let _ = world.remove::<BridgeGizmo>(entity);
}

fn noop_on_removed(
    _owner: &pulsar_reflection::RuntimeComponentOwner,
    _context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
) {
}

fn noop_refresh(_world: &mut World, _entity: pulsar_scenedb::Entity) {}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::WorldComponentRegistration {
        class_name: "BridgeGizmo",
        component_type: pulsar_scenedb::component_id::<BridgeGizmo>,
        hydrate: bridge_gizmo_hydrate,
        remove: bridge_gizmo_remove,
        dispatch: |world, entity, _owner, _idx, _ctx| world.get::<BridgeGizmo>(entity).is_some(),
        get_as_engine_class: bridge_gizmo_get,
        get_as_engine_class_mut: bridge_gizmo_get_mut,
        on_removed: noop_on_removed,
        refresh_gpu_mirror: noop_refresh,
    }
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::EngineClassRegistration {
        name: "BridgeGizmo",
        category: None,
        constructor: <BridgeGizmo as EngineClass>::create_default,
        from_json: Some(|data: &serde_json::Value| {
            serde_json::from_value::<BridgeGizmo>(data.clone())
                .map(|g| Box::new(g) as Box<dyn EngineClass>)
                .map_err(|e| e.to_string())
        }),
    }
}

// ── helpers ────────────────────────────────────────────────────────────────

fn snap(stable_id: &str, parent: Option<&str>) -> ObjectSnapshot {
    ObjectSnapshot {
        stable_id: stable_id.to_string(),
        name: stable_id.to_string(),
        parent: parent.map(str::to_string),
        transform: Transform::default(),
        visibility: Visibility::default(),
        object_type: engine_backend::scene::ObjectType::Empty,
        render_props: RenderProps::default(),
    }
}

/// A saved session: snapshots + the serialized reference a graph held.
fn session_with_door_and_chest() -> (Vec<ObjectSnapshot>, SerializedComponentRef) {
    let mut store =
        WorldSceneStore::load_from_snapshots(&[snap("door", None), snap("chest", None)]).unwrap();
    let door = store.entity_for("door").unwrap();

    // The gameplay state a graph would reference and mutate.
    store.world_mut().insert(door, BridgeGizmo { charge: 10 });

    let r = ComponentRef::live(door.into(), "BridgeGizmo");
    let saved = r.to_serialized(&store).expect("door has a stable id");

    (store.to_snapshots(), saved)
}

// ── the #639 acceptance tests ──────────────────────────────────────────────

/// Save -> load -> the same reference still targets the intended component:
/// writes through the RESOLVED ref mutate the reloaded object, not whatever
/// inherited the old entity bits.
#[test]
fn reference_survives_save_load_and_still_targets_the_intended_component() {
    let (snapshots, saved) = session_with_door_and_chest();

    // Reload into a fresh store -- entity bits are free to differ entirely.
    let mut store = WorldSceneStore::load_from_snapshots(&snapshots).unwrap();
    let door = store.entity_for("door").unwrap();
    let chest = store.entity_for("chest").unwrap();
    store.world_mut().insert(door, BridgeGizmo { charge: 10 });
    store.world_mut().insert(chest, BridgeGizmo { charge: 99 });

    let resolved = saved
        .resolve(&store)
        .expect("reference resolves after load");
    assert_eq!(resolved.class_name, "BridgeGizmo");
    assert_eq!(resolved.component_index, 0);

    resolved
        .set_property(store.world_mut(), "charge", serde_json::json!(42))
        .expect("writes");

    let door = store.entity_for("door").unwrap();
    let chest = store.entity_for("chest").unwrap();
    assert_eq!(store.world().get::<BridgeGizmo>(door).unwrap().charge, 42);
    assert_eq!(
        store.world().get::<BridgeGizmo>(chest).map(|g| g.charge),
        Some(99),
        "the sibling was never touched"
    );

    // And the shared-store handle pattern works end to end (#634 contract):
    let shared: Arc<RwLock<WorldSceneStore>> = Arc::new(RwLock::new(store));
    let again = saved.resolve(&*shared.read()).unwrap();
    assert_eq!(
        again
            .get_property(&shared.read().world(), "charge")
            .unwrap(),
        serde_json::json!(42)
    );
}

/// Deleting the target reports typed ReferenceLost after load -- never a
/// silent rebinding onto another object that happens to occupy nearby slots.
#[test]
fn deleted_target_reports_reference_lost_after_load() {
    let (mut snapshots, saved) = session_with_door_and_chest();

    // The "door" object no longer exists in the next session's file.
    snapshots.retain(|s| s.stable_id != "door");
    let store = WorldSceneStore::load_from_snapshots(&snapshots).unwrap();

    assert_eq!(
        saved.resolve(&store),
        Err(ResolveRefError::ReferenceLost {
            stable_id: "door".into()
        })
    );
}

/// Hierarchy edits between sessions don't disturb references: reparenting
/// changes nothing about stable ids, so resolution still lands on target --
/// resolution is lazy, per access, against the CURRENT table.
#[test]
fn reparenting_between_sessions_does_not_disturb_references() {
    let (snapshots, saved) = session_with_door_and_chest();

    // Next session the editor moved "chest" under "door" before loading.
    let mut edited = snapshots.clone();
    for snapshot in edited.iter_mut() {
        if snapshot.stable_id == "chest" {
            snapshot.parent = Some("door".into());
        }
    }
    let store = WorldSceneStore::load_from_snapshots(&edited).unwrap();

    let resolved = saved
        .resolve(&store)
        .expect("reparenting must not lose references");
    assert_eq!(resolved.actor().entity(), store.entity_for("door").unwrap());
}

/// Stale-session references (target despawned BEFORE freezing) fail at
/// freeze time rather than persisting garbage.
#[test]
fn freezing_a_despawned_target_is_a_typed_error() {
    let mut store =
        WorldSceneStore::load_from_snapshots(&[snap("door", None), snap("chest", None)]).unwrap();
    let door = store.entity_for("door").unwrap();
    store.despawn(door);

    let dangling = ComponentRef::live(door.into(), "BridgeGizmo");
    assert_eq!(
        dangling.to_serialized(&store),
        Err(ResolveRefError::ReferenceLost {
            stable_id: String::new()
        })
    );
    // Per-access staleness stays the #641 taxonomy:
    assert!(matches!(
        dangling.validate(store.world()),
        Err(ScriptRefError::ReferenceDespawned { .. })
    ));
}
