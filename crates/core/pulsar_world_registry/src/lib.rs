//! Bridges reflection-typed, [`pulsar_reflection::ComponentRuntimeBehavior`]-
//! implementing components into `pulsar_scenedb::World` (Pulsar-Native#555/
//! #556, Phase B4/B5 of the SceneDB + Helio + reflection/properties-panel
//! unification -- see [Pulsar-Native#561](https://github.com/Far-Beyond-Pulsar/Pulsar-Native/issues/561)).
//!
//! ## What this solves
//!
//! Before this crate, a component's *live* runtime shape was always
//! `serde_json::Value`: `HelioRenderer::sync_scene` re-deserialized every
//! component's JSON into its typed struct on *every rendered frame* (via
//! `apply_runtime_behavior_for_class`), even though `#[register_runtime_behavior]`
//! (Phase B2) already made `ComponentRuntimeBehavior::sync_component` itself
//! typed -- the JSON round trip was purely an artifact of the dispatch
//! boundary, not the trait.
//!
//! `#[register_world_component]` (`engine_class_derive`) lets a component opt
//! into this crate's registry, which provides:
//! - **`hydrate`**: deserialize a component's JSON *once*, when it's
//!   actually edited (`SceneDatabase`'s component-mutation hook), and insert
//!   the typed value into `World` at that entity.
//! - **`dispatch`**: read the typed value already sitting in `World` and call
//!   `ComponentRuntimeBehavior::sync_component` directly -- no JSON, no
//!   `serde_json::from_value` -- on the render hot path.
//! - **`remove`**: drop the typed value when the component is deleted,
//!   disabled, or its owning object is despawned.
//!
//! ## Why this crate exists instead of extending `pulsar_reflection` directly
//!
//! `pulsar_reflection` is a separate repository, also used by non-SceneDB
//! contexts (the Blueprint and shader editors reuse its property-reflection
//! machinery), so it deliberately has no dependency on `pulsar_scenedb`.
//! `pulsar_scenedb` is *also* a separate repository, and Pulsar-Native's pin
//! of it is currently stuck (Pulsar-Native#560, blocked on
//! [SceneDB#46](https://github.com/Far-Beyond-Pulsar/SceneDB/issues/46)) --
//! landing new SceneDB-repo code and getting it pulled into this workspace
//! is real, avoidable friction right now. Every actual consumer here
//! (`pulsar_rendering`, `pulsar_physics`, `engine_backend`, `ui_level_editor`)
//! already depends on both `pulsar_scenedb` and `pulsar_reflection` directly,
//! so a small Pulsar-Native-internal crate sitting alongside them -- itself
//! depending on both -- is a clean fit with no cross-repo coordination
//! needed at all.
//!
//! ## Why a separate registry from `RuntimeBehaviorRegistration`
//!
//! `pulsar_reflection::RuntimeBehaviorRegistration`/`apply_runtime_behavior_for_class`
//! stay exactly as they are (JSON-based) -- they're still the dispatch path
//! for anything that only has JSON on hand (`pulsar_scene::SceneLoader`, and
//! any component that hasn't been migrated onto this registry yet, e.g. the
//! rest of B5's list before it lands). Migration is opt-in and incremental,
//! one component at a time, which is exactly why this is a sibling registry
//! rather than a breaking change to the existing one.

// Re-exported so `#[register_world_component]` (`engine_class_derive`) can
// emit `pulsar_world_registry::inventory::submit! { .. }` in the calling
// crate without that crate needing its own direct `inventory` dependency --
// same pattern `pulsar_reflection` already uses for `RuntimeBehaviorRegistration`.
pub use inventory;

use pulsar_reflection::{ComponentRuntimeContext, EngineClass, RuntimeComponentOwner};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

/// One registered component class's `World` bridge. Populated by
/// `#[register_world_component]` (`engine_class_derive`) -- not constructed
/// by hand.
pub struct WorldComponentRegistration {
    pub class_name: &'static str,
    /// Deserialize `data` and insert/overwrite this class's typed component
    /// on `entity`. Called once per edit (from `SceneDatabase`'s component
    /// mutation hook), never from the render loop.
    pub hydrate: fn(&mut World, Entity, &Value) -> Result<(), String>,
    /// Remove this class's typed component from `entity`, if present.
    /// Called when the component is deleted, disabled, or its owning object
    /// is despawned.
    pub remove: fn(&mut World, Entity),
    /// Dispatch `ComponentRuntimeBehavior::sync_component` using the typed
    /// value already in `World` -- no JSON deserialize on this path at all.
    /// Returns `false` (and does nothing) if `entity` doesn't have this
    /// component in `World` (not hydrated yet); callers should fall back to
    /// `pulsar_reflection::apply_runtime_behavior_for_class` in that case,
    /// which still works off the JSON channel unconditionally.
    pub dispatch: fn(
        &World,
        Entity,
        &RuntimeComponentOwner,
        usize,
        &mut dyn ComponentRuntimeContext,
    ) -> bool,
    /// Borrow the typed value already in `World` as `&dyn EngineClass` --
    /// the properties panel's *read* path. No JSON, no throwaway `Default`
    /// instance: this is the one real, live value.
    pub get_as_engine_class: fn(&World, Entity) -> Option<&dyn EngineClass>,
    /// Borrow the typed value already in `World` as `&mut dyn EngineClass`
    /// -- the properties panel's *write* path. Apply a `PropertyMetadata`
    /// setter closure straight to this reference to mutate the one real
    /// value in place; there is no second copy to keep in sync afterward.
    pub get_as_engine_class_mut: fn(&mut World, Entity) -> Option<&mut dyn EngineClass>,
}

inventory::collect!(WorldComponentRegistration);

fn find(class_name: &str) -> Option<&'static WorldComponentRegistration> {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .find(|r| r.class_name == class_name)
}

/// Hydrate `class_name`'s typed component from `data` onto `entity`. Returns
/// `Ok(false)` if `class_name` isn't registered here (not migrated yet, or
/// not a real component class) -- not an error, it just means the JSON
/// channel stays authoritative for this class. Returns `Err` only if
/// `class_name` *is* registered but `data` failed to deserialize.
pub fn hydrate_world_component_for_class(
    class_name: &str,
    world: &mut World,
    entity: Entity,
    data: &Value,
) -> Result<bool, String> {
    match find(class_name) {
        Some(registration) => (registration.hydrate)(world, entity, data).map(|()| true),
        None => Ok(false),
    }
}

/// Remove `class_name`'s typed component from `entity`, if that class is
/// registered here. Returns `false` if `class_name` isn't registered.
pub fn remove_world_component_for_class(class_name: &str, world: &mut World, entity: Entity) -> bool {
    match find(class_name) {
        Some(registration) => {
            (registration.remove)(world, entity);
            true
        }
        None => false,
    }
}

/// Dispatch `class_name`'s `ComponentRuntimeBehavior::sync_component`
/// directly off `entity`'s typed `World` value, if that class is registered
/// here and hydrated on `entity`. Returns `false` otherwise -- callers
/// should fall back to `pulsar_reflection::apply_runtime_behavior_for_class`.
pub fn dispatch_world_component_for_class(
    class_name: &str,
    world: &World,
    entity: Entity,
    owner: &RuntimeComponentOwner,
    component_index: usize,
    context: &mut dyn ComponentRuntimeContext,
) -> bool {
    match find(class_name) {
        Some(registration) => (registration.dispatch)(world, entity, owner, component_index, context),
        None => false,
    }
}

/// Borrow `class_name`'s typed value already in `World` as `&dyn EngineClass`
/// for reading -- the properties panel's read path (Pulsar-Native#561).
/// `None` if `class_name` isn't registered here, or `entity` doesn't have
/// this component in `World` yet; callers should fall back to whatever
/// non-live source they have (JSON, a `Default` instance) in that case.
pub fn get_world_component_as_engine_class<'w>(
    class_name: &str,
    world: &'w World,
    entity: Entity,
) -> Option<&'w dyn EngineClass> {
    (find(class_name)?.get_as_engine_class)(world, entity)
}

/// Borrow `class_name`'s typed value already in `World` as
/// `&mut dyn EngineClass` for direct in-place editing -- the properties
/// panel's write path. Apply a property's setter closure straight to this
/// reference: it mutates the one real `World`-resident component, so there
/// is nothing to write back afterward. `None` under the same conditions as
/// [`get_world_component_as_engine_class`].
pub fn get_world_component_as_engine_class_mut<'w>(
    class_name: &str,
    world: &'w mut World,
    entity: Entity,
) -> Option<&'w mut dyn EngineClass> {
    (find(class_name)?.get_as_engine_class_mut)(world, entity)
}

/// Every currently-registered `World`-backed class name. `SceneDatabase`
/// uses this to know which classes to check for removal when an object's
/// component list changes -- a class present in `World` from a previous
/// hydration but no longer in the object's current enabled component list
/// needs `remove` called, and this is how it finds out which classes to
/// even ask about.
pub fn registered_world_component_classes() -> impl Iterator<Item = &'static str> {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .map(|r| r.class_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Clone, Debug, PartialEq, Default, serde::Deserialize)]
    struct TestComponent {
        value: i32,
    }

    // Minimal hand-written `EngineClass` impl -- in real components this
    // comes from `#[derive(EngineClass)]`, but this test struct exists only
    // to exercise `WorldComponentRegistration`'s plumbing, not the
    // reflection macro.
    impl EngineClass for TestComponent {
        fn class_name() -> &'static str {
            "TestComponent"
        }
        fn get_properties(&self) -> Vec<pulsar_reflection::PropertyMetadata> {
            Vec::new()
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
    }

    fn test_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
        world.get::<TestComponent>(entity).map(|c| c as &dyn EngineClass)
    }

    fn test_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
        world.get_mut::<TestComponent>(entity).map(|c| c as &mut dyn EngineClass)
    }

    fn test_hydrate(world: &mut World, entity: Entity, data: &Value) -> Result<(), String> {
        let parsed: TestComponent = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
        world.insert(entity, parsed);
        Ok(())
    }

    fn test_remove(world: &mut World, entity: Entity) {
        let _ = world.remove::<TestComponent>(entity);
    }

    fn test_dispatch(
        world: &World,
        entity: Entity,
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _context: &mut dyn ComponentRuntimeContext,
    ) -> bool {
        world.get::<TestComponent>(entity).is_some()
    }

    inventory::submit! {
        WorldComponentRegistration {
            class_name: "TestComponent",
            hydrate: test_hydrate,
            remove: test_remove,
            dispatch: test_dispatch,
            get_as_engine_class: test_get,
            get_as_engine_class_mut: test_get_mut,
        }
    }

    struct DummyContext {
        subsystems: pulsar_reflection::Subsystems,
    }

    impl ComponentRuntimeContext for DummyContext {
        fn subsystems_mut(&mut self) -> &mut pulsar_reflection::Subsystems {
            &mut self.subsystems
        }
        fn project_root(&self) -> &std::path::Path {
            std::path::Path::new(".")
        }
        fn report_error(&mut self, _message: String) {}
    }

    fn dummy_context() -> DummyContext {
        DummyContext { subsystems: pulsar_reflection::Subsystems::new() }
    }

    fn dummy_owner(props: &HashMap<String, Value>) -> RuntimeComponentOwner<'_> {
        RuntimeComponentOwner {
            scene_object_id: "test",
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            props,
        }
    }

    #[test]
    fn registered_test_component_is_discoverable() {
        assert!(registered_world_component_classes().any(|c| c == "TestComponent"));
    }

    #[test]
    fn hydrate_dispatch_remove_round_trip() {
        let mut world = World::new();
        let entity = world.spawn();
        let props = HashMap::new();
        let mut ctx = dummy_context();

        // Not hydrated yet -- dispatch is a clean no-op, not a panic.
        assert!(!dispatch_world_component_for_class(
            "TestComponent", &world, entity, &dummy_owner(&props), 0, &mut ctx
        ));

        let hydrated = hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 42}),
        )
        .unwrap();
        assert!(hydrated);
        assert_eq!(world.get::<TestComponent>(entity), Some(&TestComponent { value: 42 }));

        assert!(dispatch_world_component_for_class(
            "TestComponent", &world, entity, &dummy_owner(&props), 0, &mut ctx
        ));

        assert!(remove_world_component_for_class("TestComponent", &mut world, entity));
        assert!(world.get::<TestComponent>(entity).is_none());
    }

    #[test]
    fn live_get_mut_edits_the_one_real_world_value_directly() {
        let mut world = World::new();
        let entity = world.spawn();

        // Not hydrated yet -- no live value to edit.
        assert!(get_world_component_as_engine_class_mut("TestComponent", &mut world, entity)
            .is_none());
        assert!(get_world_component_as_engine_class("TestComponent", &world, entity).is_none());

        hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 1}),
        )
        .unwrap();

        // Mutate through the `&mut dyn EngineClass` path -- this must be the
        // same storage `world.get::<TestComponent>` sees afterward, not a
        // copy: no serialize/deserialize anywhere in this path.
        {
            let instance =
                get_world_component_as_engine_class_mut("TestComponent", &mut world, entity)
                    .expect("hydrated component should be live-accessible");
            let concrete = instance.as_any_mut().downcast_mut::<TestComponent>().unwrap();
            concrete.value = 99;
        }

        assert_eq!(world.get::<TestComponent>(entity), Some(&TestComponent { value: 99 }));
        let read_back = get_world_component_as_engine_class("TestComponent", &world, entity)
            .expect("component should still be live-accessible for reading");
        assert_eq!(
            read_back.as_any().downcast_ref::<TestComponent>(),
            Some(&TestComponent { value: 99 })
        );
    }

    #[test]
    fn unregistered_class_is_a_clean_no_op_everywhere() {
        let mut world = World::new();
        let entity = world.spawn();
        let props = HashMap::new();
        let mut ctx = dummy_context();

        assert_eq!(
            hydrate_world_component_for_class(
                "NotRegistered", &mut world, entity, &serde_json::json!({})
            )
            .unwrap(),
            false
        );
        assert!(!remove_world_component_for_class("NotRegistered", &mut world, entity));
        assert!(!dispatch_world_component_for_class(
            "NotRegistered", &world, entity, &dummy_owner(&props), 0, &mut ctx
        ));
    }

    #[test]
    fn hydrate_surfaces_malformed_json_as_an_error() {
        let mut world = World::new();
        let entity = world.spawn();

        let err = hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!("not an object"),
        )
        .unwrap_err();
        assert!(!err.is_empty());
        // A failed hydrate must not leave a half-inserted component behind.
        assert!(world.get::<TestComponent>(entity).is_none());
    }

    #[test]
    fn hydrate_overwrites_a_previous_value() {
        let mut world = World::new();
        let entity = world.spawn();

        hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 1}),
        )
        .unwrap();
        hydrate_world_component_for_class(
            "TestComponent", &mut world, entity, &serde_json::json!({"value": 2}),
        )
        .unwrap();

        assert_eq!(world.get::<TestComponent>(entity), Some(&TestComponent { value: 2 }));
    }
}
