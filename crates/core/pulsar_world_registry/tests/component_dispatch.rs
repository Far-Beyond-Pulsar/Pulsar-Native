//! #643 acceptance: a caller holding only `(world, entity ids, strings)`
//! can execute any registered reflected method against authoritative state
//! -- no concrete component type named anywhere in the call chain -- and
//! every failure mode arrives as a typed [`ScriptRefError`], never a panic
//! (the generated caller closures DO panic on bad args; the dispatcher must
//! refuse them first).

use pulsar_reflection::{
    ComponentMethodRegistration, EngineClass, EngineClassRegistration, MethodMetadata,
    MethodParameter, MethodReturnType, MethodType, PropertyMetadata, RuntimeTypeInfo,
    RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use pulsar_world_registry::{
    get_component_property, get_component_property_boxed, invoke_component_method,
    set_component_property, set_component_property_boxed, ScriptRefError,
};

// ── one hand-registered test class exercising the full pipeline ────────────

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct DispatchGizmo {
    charges: i32,
}

impl EngineClass for DispatchGizmo {
    fn class_name() -> &'static str {
        "DispatchGizmo"
    }

    fn get_properties(&self) -> Vec<PropertyMetadata> {
        let info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY
            .get::<i32>()
            .expect("i32 prim registered");
        vec![PropertyMetadata {
            name: "charges",
            display_name: "Charges".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info: info,
            getter: Box::new(|c: &dyn EngineClass| {
                Box::new(c.as_any().downcast_ref::<DispatchGizmo>().unwrap().charges)
            }),
            setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                if let Some(v) = v.downcast_ref::<i32>() {
                    c.as_any_mut()
                        .downcast_mut::<DispatchGizmo>()
                        .unwrap()
                        .charges = *v;
                }
            }),
        }]
    }

    fn get_methods() -> Vec<MethodMetadata> {
        let info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY
            .get::<i32>()
            .expect("i32 prim registered");
        vec![MethodMetadata {
            name: "add_charges",
            display_name: "Add Charges".into(),
            category: None,
            params: vec![MethodParameter {
                name: "amount",
                type_info: info,
            }],
            return_type: Some(MethodReturnType { type_info: info }),
            // Deliberately NOT Pure: mutates state (#645's purity policy).
            method_type: MethodType::Fn,
            caller: Box::new(
                |c: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                    let amount = args
                        .first()
                        .and_then(|a| a.downcast_ref::<i32>())
                        .copied()?;
                    let gizmo = c.as_any_mut().downcast_mut::<DispatchGizmo>()?;
                    gizmo.charges += amount;
                    Some(Box::new(gizmo.charges))
                },
            ),
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

    fn to_json(&self) -> Result<Value, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}

fn gizmo_hydrate(world: &mut World, entity: Entity, data: &Value) -> Result<(), String> {
    let parsed: DispatchGizmo = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    world.insert(entity, parsed);
    Ok(())
}

fn gizmo_remove(world: &mut World, entity: Entity) {
    let _ = world.remove::<DispatchGizmo>(entity);
}

fn gizmo_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
    world
        .get::<DispatchGizmo>(entity)
        .map(|c| c as &dyn EngineClass)
}

fn gizmo_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
    // Same `Mut`-guard unwrap as the generated shims: writes through here
    // count as real mutations for subscriptions/GPU mirrors.
    world
        .get_mut::<DispatchGizmo>(entity)
        .map(|c| c.into_inner() as &mut dyn EngineClass)
}

fn gizmo_methods() -> Vec<MethodMetadata> {
    <DispatchGizmo as EngineClass>::get_methods()
}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::WorldComponentRegistration {
        class_name: "DispatchGizmo",
        component_type: pulsar_scenedb::component_id::<DispatchGizmo>,
        hydrate: gizmo_hydrate,
        remove: gizmo_remove,
        dispatch: |world, entity, _owner, _index, _ctx| world.get::<DispatchGizmo>(entity).is_some(),
        get_as_engine_class: gizmo_get,
        get_as_engine_class_mut: gizmo_get_mut,
        on_removed: |_owner, _context| {},
        refresh_gpu_mirror: |_world, _entity| {},
    }
}

pulsar_reflection::inventory::submit! {
    EngineClassRegistration {
        name: "DispatchGizmo",
        category: None,
        constructor: <DispatchGizmo as EngineClass>::create_default,
        from_json: None,
    }
}

pulsar_reflection::inventory::submit! {
    ComponentMethodRegistration {
        class_name: "DispatchGizmo",
        methods: gizmo_methods,
    }
}

// ── fixtures ───────────────────────────────────────────────────────────────

fn hydrated_world(charges: i32) -> (World, Entity) {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, DispatchGizmo { charges });
    (world, entity)
}

// ── #643 acceptance ────────────────────────────────────────────────────────

/// A string-and-ids caller executes a real reflected method against the live
/// World value; args marshal in, the return value marshals out.
#[test]
fn native_call_runs_against_the_live_typed_instance() {
    let (mut world, e) = hydrated_world(3);

    let returned = invoke_component_method(
        &mut world,
        e,
        "DispatchGizmo",
        0,
        "add_charges",
        vec![Box::new(7i32)],
    )
    .unwrap()
    .expect("method returns new total");

    assert_eq!(returned.downcast_ref::<i32>(), Some(&10));
    assert_eq!(world.get::<DispatchGizmo>(e).unwrap().charges, 10);
}

/// Wrong arity is a typed error BEFORE any caller runs -- the generated
/// closure would panic on the missing argument, so this also proves the
/// dispatcher's validation gate works.
#[test]
fn wrong_argument_count_is_typed_not_a_panic() {
    let (mut world, e) = hydrated_world(3);

    let err = invoke_component_method(&mut world, e, "DispatchGizmo", 0, "add_charges", vec![])
        .unwrap_err();

    assert_eq!(
        err,
        ScriptRefError::ArgumentCount {
            class_name: "DispatchGizmo".into(),
            method: "add_charges".into(),
            expected: 1,
            got: 0,
        }
    );
    assert_eq!(
        world.get::<DispatchGizmo>(e).unwrap().charges,
        3,
        "nothing dispatched"
    );
}

/// Wrong argument TYPE is typed (naming both sides), again before dispatch.
#[test]
fn wrong_argument_type_is_typed_not_a_panic() {
    let (mut world, e) = hydrated_world(3);

    let err = invoke_component_method(
        &mut world,
        e,
        "DispatchGizmo",
        0,
        "add_charges",
        vec![Box::new("nope".to_string())],
    )
    .unwrap_err();

    match err {
        ScriptRefError::ArgumentType {
            index,
            param,
            expected,
            found,
            ..
        } => {
            assert_eq!(index, 0);
            assert_eq!(param, "amount");
            assert_eq!(expected, "i32");
            assert_eq!(
                found, "String",
                "found names the registered type, not a raw TypeId"
            );
        }
        other => panic!("expected ArgumentType, got {other:?}"),
    }
    assert_eq!(world.get::<DispatchGizmo>(e).unwrap().charges, 3);
}

/// Despawned targets are ordinary typed staleness (#641 contract holds at
/// the dispatcher boundary).
#[test]
fn despawned_entity_is_reference_despawned() {
    let (mut world, e) = hydrated_world(3);
    world.despawn(e);

    let err = invoke_component_method(
        &mut world,
        e,
        "DispatchGizmo",
        0,
        "add_charges",
        vec![Box::new(1i32)],
    )
    .unwrap_err();

    assert!(matches!(err, ScriptRefError::ReferenceDespawned { .. }));
}

/// Name-level failures stay distinct: unregistered class vs unknown method.
#[test]
fn unknown_class_and_method_are_distinct_typed_errors() {
    let (mut world, e) = hydrated_world(3);

    let err = invoke_component_method(&mut world, e, "NeverRegistered", 0, "anything", vec![])
        .unwrap_err();
    assert_eq!(
        err,
        ScriptRefError::UnregisteredClass("NeverRegistered".into())
    );

    let err =
        invoke_component_method(&mut world, e, "DispatchGizmo", 0, "nope", vec![]).unwrap_err();
    assert_eq!(
        err,
        ScriptRefError::UnknownMethod {
            class_name: "DispatchGizmo".into(),
            method: "nope".into()
        }
    );
}

/// Alive but never hydrated: ComponentMissing, not a bridge failure.
#[test]
fn unhydrated_component_is_component_missing() {
    let mut world = World::new();
    let e = world.spawn();

    let err = invoke_component_method(
        &mut world,
        e,
        "DispatchGizmo",
        0,
        "add_charges",
        vec![Box::new(1i32)],
    )
    .unwrap_err();
    assert!(matches!(err, ScriptRefError::ComponentMissing { .. }));
}

// ── property equivalents composing the same routing ────────────────────────

/// JSON get/set round trip through PropertyMetadata closures; the setter
/// rides the typed bridge (Mut-guard events fire exactly like panel edits).
#[test]
fn property_json_round_trip_through_the_live_value() {
    let (mut world, e) = hydrated_world(5);

    set_component_property(
        &mut world,
        e,
        "DispatchGizmo",
        0,
        "charges",
        serde_json::json!(11),
    )
    .unwrap();
    assert_eq!(world.get::<DispatchGizmo>(e).unwrap().charges, 11);

    let json = get_component_property(&world, e, "DispatchGizmo", 0, "charges").unwrap();
    assert_eq!(json, serde_json::json!(11));
}

/// The boxed hot path skips JSON entirely; a wrongly-typed box is REFUSED
/// (typed error), never silently ignored by the setter's downcast.
#[test]
fn boxed_property_set_skips_json_and_refuses_wrong_types() {
    let (mut world, e) = hydrated_world(1);

    set_component_property_boxed(
        &mut world,
        e,
        "DispatchGizmo",
        0,
        "charges",
        Box::new(42i32),
    )
    .unwrap();
    assert_eq!(world.get::<DispatchGizmo>(e).unwrap().charges, 42);

    let got = get_component_property_boxed(&world, e, "DispatchGizmo", 0, "charges").unwrap();
    assert_eq!(got.downcast_ref::<i32>(), Some(&42));

    let err =
        set_component_property_boxed(&mut world, e, "DispatchGizmo", 0, "charges", Box::new(7i64))
            .unwrap_err();
    assert!(matches!(err, ScriptRefError::ArgumentType { .. }));
    assert_eq!(
        world.get::<DispatchGizmo>(e).unwrap().charges,
        42,
        "refused write"
    );
}

/// Malformed JSON is a typed Marshalling error and writes nothing.
#[test]
fn malformed_property_json_writes_nothing() {
    let (mut world, e) = hydrated_world(9);

    let err = set_component_property(
        &mut world,
        e,
        "DispatchGizmo",
        0,
        "charges",
        serde_json::json!("not-a-number"),
    )
    .unwrap_err();
    assert!(matches!(err, ScriptRefError::Marshalling { .. }));
    assert_eq!(world.get::<DispatchGizmo>(e).unwrap().charges, 9);
}

/// Panel-parity identity at this layer: only index 0 (live-typed) is
/// addressable; duplicate records belong to the object-model store seam.
#[test]
fn nonzero_component_index_is_instance_missing_for_properties() {
    let (world, e) = hydrated_world(2);

    let err = get_component_property(&world, e, "DispatchGizmo", 1, "charges").unwrap_err();
    assert!(matches!(err, ScriptRefError::InstanceMissing { .. }));

    let err = get_component_property(&world, e, "DispatchGizmo", 0, "nope").unwrap_err();
    assert!(matches!(err, ScriptRefError::UnknownProperty { .. }));
}
