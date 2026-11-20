//! Registry plumbing tests over a hand-written component class.

use pulsar_reflection::EngineClass;
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use crate::{
    get_world_component_as_engine_class, get_world_component_as_engine_class_mut,
    hydrate_world_component_for_class, registered_world_component_classes,
    remove_world_component_for_class, EngineClassMut, Presentation, WorldComponentRegistration,
};

#[derive(Clone, Debug, PartialEq, Default, serde::Deserialize)]
pub(crate) struct TestComponent {
    pub(crate) value: i32,
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
    world
        .get::<TestComponent>(entity)
        .map(|c| c as &dyn EngineClass)
}

fn test_get_mut(world: &mut World, entity: Entity) -> Option<EngineClassMut<'_>> {
    EngineClassMut::of::<TestComponent>(world, entity)
}

fn test_default() -> Box<dyn std::any::Any + Send + Sync> {
    Box::new(TestComponent { value: 0 })
}

fn test_decode(data: &Value) -> Result<Box<dyn std::any::Any + Send + Sync>, String> {
    let parsed: TestComponent = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    Ok(Box::new(parsed))
}

fn test_clone(value: &dyn std::any::Any) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    value
        .downcast_ref::<TestComponent>()
        .map(|v| Box::new(v.clone()) as Box<dyn std::any::Any + Send + Sync>)
}

fn test_remove(world: &mut World, entity: Entity) {
    let _ = world.remove::<TestComponent>(entity);
}

fn test_property_written(_value: &mut dyn EngineClass, _property: Option<&str>) {}

inventory::submit! {
    WorldComponentRegistration {
        origin: env!("CARGO_PKG_NAME"),
        class_name: "TestComponent",
        component_type: pulsar_scenedb::component_id::<TestComponent>,
        default_value: test_default,
        decode: test_decode,
        clone_value: test_clone,
        value_as_engine_class: |_| None,
        value_as_engine_class_mut: |_| None,
        register_erased: pulsar_scenedb::register_component::<TestComponent>,
        remove: test_remove,
        get_as_engine_class: test_get,
        get_as_engine_class_mut: test_get_mut,
        property_written: test_property_written,
        requires: &[],
        presentation: Presentation::Component,
    }
}

#[test]
fn registered_test_component_is_discoverable() {
    assert!(registered_world_component_classes().any(|c| c == "TestComponent"));
}

#[test]
fn hydrate_remove_round_trip() {
    let mut world = World::new();
    let entity = world.spawn();
    let hydrated = hydrate_world_component_for_class(
        "TestComponent",
        &mut world,
        entity,
        &serde_json::json!({"value": 42}),
    )
    .unwrap();
    assert!(hydrated);
    assert_eq!(
        world.get::<TestComponent>(entity),
        Some(&TestComponent { value: 42 })
    );

    assert!(remove_world_component_for_class(
        "TestComponent",
        &mut world,
        entity
    ));
    assert!(world.get::<TestComponent>(entity).is_none());
}

/// The write is reported when the guard drops, after the edit, and a
/// guard that was only read through reports nothing.
#[test]
fn the_write_guard_reports_after_the_edit_and_only_when_written() {
    use pulsar_scenedb::ComponentChangeKind;

    let mut world = World::new();
    let entity = world.spawn();
    hydrate_world_component_for_class(
        "TestComponent",
        &mut world,
        entity,
        &serde_json::json!({"value": 1}),
    )
    .unwrap();
    let mut cursor = world.open_change_cursor::<TestComponent>();
    let mut changes = Vec::new();

    {
        let guard =
            get_world_component_as_engine_class_mut("TestComponent", &mut world, entity).unwrap();
        let _ = guard.to_json();
    }
    let _ = world.read_changes(&mut cursor, &mut changes);
    assert!(changes.is_empty(), "a read is not a mutation");

    let mut guard =
        get_world_component_as_engine_class_mut("TestComponent", &mut world, entity).unwrap();
    guard
        .as_any_mut()
        .downcast_mut::<TestComponent>()
        .unwrap()
        .value = 7;
    drop(guard);
    let _ = world.read_changes(&mut cursor, &mut changes);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].kind, ComponentChangeKind::Mutated);
    assert_eq!(
        world.get::<TestComponent>(entity),
        Some(&TestComponent { value: 7 })
    );
}

#[test]
fn live_get_mut_edits_the_one_real_world_value_directly() {
    let mut world = World::new();
    let entity = world.spawn();

    // Not hydrated yet -- no live value to edit.
    assert!(get_world_component_as_engine_class_mut("TestComponent", &mut world, entity).is_none());
    assert!(get_world_component_as_engine_class("TestComponent", &world, entity).is_none());

    hydrate_world_component_for_class(
        "TestComponent",
        &mut world,
        entity,
        &serde_json::json!({"value": 1}),
    )
    .unwrap();

    // Mutate through the `&mut dyn EngineClass` path -- this must be the
    // same storage `world.get::<TestComponent>` sees afterward, not a
    // copy: no serialize/deserialize anywhere in this path.
    {
        let mut instance =
            get_world_component_as_engine_class_mut("TestComponent", &mut world, entity)
                .expect("hydrated component should be live-accessible");
        let concrete = instance
            .as_any_mut()
            .downcast_mut::<TestComponent>()
            .unwrap();
        concrete.value = 99;
    }

    assert_eq!(
        world.get::<TestComponent>(entity),
        Some(&TestComponent { value: 99 })
    );
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

    assert_eq!(
        hydrate_world_component_for_class(
            "NotRegistered",
            &mut world,
            entity,
            &serde_json::json!({})
        )
        .unwrap(),
        false
    );
    assert!(!remove_world_component_for_class(
        "NotRegistered",
        &mut world,
        entity
    ));
}

#[test]
fn hydrate_surfaces_malformed_json_as_an_error() {
    let mut world = World::new();
    let entity = world.spawn();

    let err = hydrate_world_component_for_class(
        "TestComponent",
        &mut world,
        entity,
        &serde_json::json!("not an object"),
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
        "TestComponent",
        &mut world,
        entity,
        &serde_json::json!({"value": 1}),
    )
    .unwrap();
    hydrate_world_component_for_class(
        "TestComponent",
        &mut world,
        entity,
        &serde_json::json!({"value": 2}),
    )
    .unwrap();

    assert_eq!(
        world.get::<TestComponent>(entity),
        Some(&TestComponent { value: 2 })
    );
}
