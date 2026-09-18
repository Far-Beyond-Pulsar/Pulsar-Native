//! #639 integration: script references survive save/load through a REAL
//! save/load round trip of the scene world.
//!
//! Registers one small component class (`BridgeGizmo`) into both registries
//! so the full hydrate/edit path runs exactly as it would for a real
//! component, without depending on renderer-side classes' property shapes.

use std::sync::Arc;

use engine_backend::scene::{SceneWorldExt, SharedScene, SpawnObject};
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

/// A saved object list: `(stable id, parent stable id)` in parent-before-child order.
type Saved = Vec<(String, Option<String>)>;

fn scene_from(saved: &Saved) -> World {
    let mut world = World::new();
    for (id, parent) in saved {
        let parent = parent.as_deref().map(|p| world.entity_for(p).unwrap());
        world
            .spawn_object(SpawnObject::new(id.as_str()).with_id(id.as_str()).with_parent(parent))
            .unwrap();
    }
    world
}

fn save(world: &World) -> Saved {
    // Parent-before-child DFS, exactly what a level writer produces.
    fn walk(world: &World, parent: Option<pulsar_scenedb::Entity>, out: &mut Saved) {
        for entity in world.children_of(parent) {
            let id = world.stable_id_of(entity).unwrap().to_string();
            let parent_id = parent.map(|p| world.stable_id_of(p).unwrap().to_string());
            out.push((id, parent_id));
            walk(world, Some(entity), out);
        }
    }
    let mut out = Vec::new();
    walk(world, None, &mut out);
    out
}

fn flat(ids: &[&str]) -> Saved {
    ids.iter().map(|id| (id.to_string(), None)).collect()
}

/// A saved session: the object list + the serialized reference a graph held.
fn session_with_door_and_chest() -> (Saved, SerializedComponentRef) {
    let mut world = scene_from(&flat(&["door", "chest"]));
    let door = world.entity_for("door").unwrap();

    // The gameplay state a graph would reference and mutate.
    world.insert(door, BridgeGizmo { charge: 10 });

    let r = ComponentRef::live(door.into(), "BridgeGizmo");
    let saved = r.to_serialized(&world).expect("door has a stable id");

    (save(&world), saved)
}

// ── the #639 acceptance tests ──────────────────────────────────────────────

/// Save -> load -> the same reference still targets the intended component:
/// writes through the RESOLVED ref mutate the reloaded object, not whatever
/// inherited the old entity bits.
#[test]
fn reference_survives_save_load_and_still_targets_the_intended_component() {
    let (saved_objects, saved) = session_with_door_and_chest();

    // Reload into a fresh world -- entity bits are free to differ entirely.
    let mut world = scene_from(&saved_objects);
    let door = world.entity_for("door").unwrap();
    let chest = world.entity_for("chest").unwrap();
    world.insert(door, BridgeGizmo { charge: 10 });
    world.insert(chest, BridgeGizmo { charge: 99 });

    let resolved = saved
        .resolve(&world)
        .expect("reference resolves after load");
    assert_eq!(resolved.class_name, "BridgeGizmo");
    assert_eq!(resolved.component_index, 0);

    resolved
        .set_property(&mut world, "charge", serde_json::json!(42))
        .expect("writes");

    let door = world.entity_for("door").unwrap();
    let chest = world.entity_for("chest").unwrap();
    assert_eq!(world.get::<BridgeGizmo>(door).unwrap().charge, 42);
    assert_eq!(
        world.get::<BridgeGizmo>(chest).map(|g| g.charge),
        Some(99),
        "the sibling was never touched"
    );

    // And the shared-scene handle pattern works end to end (#634 contract):
    let scene = pulsar_scenedb::SceneDb::new();
    let mut scene = scene;
    scene.world = world;
    let shared: SharedScene = Arc::new(RwLock::new(scene));
    let again = saved.resolve(&shared.read().world).unwrap();
    assert_eq!(
        again.get_property(&shared.read().world, "charge").unwrap(),
        serde_json::json!(42)
    );
}

/// Deleting the target reports typed ReferenceLost after load -- never a
/// silent rebinding onto another object that happens to occupy nearby slots.
#[test]
fn deleted_target_reports_reference_lost_after_load() {
    let (mut saved_objects, saved) = session_with_door_and_chest();

    // The "door" object no longer exists in the next session's file.
    saved_objects.retain(|(id, _)| id != "door");
    let world = scene_from(&saved_objects);

    assert_eq!(
        saved.resolve(&world),
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
    let (saved_objects, saved) = session_with_door_and_chest();

    // Next session the editor moved "chest" under "door" before loading.
    let mut edited = saved_objects.clone();
    for (id, parent) in edited.iter_mut() {
        if id == "chest" {
            *parent = Some("door".into());
        }
    }
    let world = scene_from(&edited);

    let resolved = saved
        .resolve(&world)
        .expect("reparenting must not lose references");
    assert_eq!(resolved.actor().entity(), world.entity_for("door").unwrap());
}

/// Stale-session references (target despawned BEFORE freezing) fail at
/// freeze time rather than persisting garbage.
#[test]
fn freezing_a_despawned_target_is_a_typed_error() {
    let mut world = scene_from(&flat(&["door", "chest"]));
    let door = world.entity_for("door").unwrap();
    world.despawn_tree(door);

    let dangling = ComponentRef::live(door.into(), "BridgeGizmo");
    assert_eq!(
        dangling.to_serialized(&world),
        Err(ResolveRefError::ReferenceLost {
            stable_id: String::new()
        })
    );
    // Per-access staleness stays the #641 taxonomy:
    assert!(matches!(
        dangling.validate(&world),
        Err(ScriptRefError::ReferenceDespawned { .. })
    ));
}
