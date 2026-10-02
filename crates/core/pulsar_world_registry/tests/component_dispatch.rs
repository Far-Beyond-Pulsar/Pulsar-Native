//! #643 acceptance: a caller holding only `(world, entity ids, strings)`
//! can execute any registered reflected method against authoritative state
//! -- no concrete component type named anywhere in the call chain -- and
//! every failure mode arrives as a typed [`ScriptRefError`], never a panic
//! (the generated caller closures DO panic on bad args; the dispatcher must
//! refuse them first).

use pulsar_reflection::{
    ComponentMethodRegistration, EngineClass, EngineClassRegistration, MethodMetadata,
    MethodParameter, MethodFlags, MethodReturnType, PropertyMetadata, RuntimeTypeInfo,
    RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

// Used by the ignored manual benchmark below. This counts allocation and
// reallocation calls in the integration-test process without adding a runtime
// dependency.
struct CountingAllocator;

static ALLOCATION_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

unsafe impl std::alloc::GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::alloc::System.alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        std::alloc::System.dealloc(ptr, layout)
    }

    unsafe fn realloc(
        &self,
        ptr: *mut u8,
        layout: std::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::alloc::System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static TEST_ALLOCATOR: CountingAllocator = CountingAllocator;

use pulsar_world_registry::{
    get_component_property, get_component_property_boxed, invoke_component_method,
    set_component_property, set_component_property_boxed, ScriptRefError,
};

thread_local! {
    /// How many times this thread built a property table (the work a
    /// throwaway `EngineClass` instance exists for).
    static PROPERTY_TABLE_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

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
        PROPERTY_TABLE_BUILDS.with(|n| n.set(n.get() + 1));
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
            flags: MethodFlags::NONE,
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

fn gizmo_get_mut(world: &mut World, entity: Entity) -> Option<pulsar_world_registry::EngineClassMut<'_>> {
    // Same guard as the generated shims: writes through it are reported to
    // subscriptions/GPU mirrors when it drops.
    pulsar_world_registry::EngineClassMut::of::<DispatchGizmo>(world, entity)
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

// ── script VM natives (script_natives.rs) ─────────────────────────────────

mod script_vm {
    use std::sync::Arc;

    use pulsar_script_vm::{
        Budget, Function, Host, Import, Instr, Module, NativeRegistry, Param, Program, Signature,
        Type, TypeRegistry, Value, Vm,
    };

    use super::{hydrated_world, DispatchGizmo};

    fn gizmo() -> Type {
        Type::component("DispatchGizmo")
    }

    #[test]
    fn world_components_are_script_components() {
        assert!(TypeRegistry::global().component("DispatchGizmo").is_some());
        let registry = NativeRegistry::with_engine_natives();
        let mut names: Vec<_> = registry.methods_for(&gizmo()).map(|n| n.name.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            [
                "DispatchGizmo::add_charges",
                "DispatchGizmo::entity",
                "DispatchGizmo::exists",
                "DispatchGizmo::get_charges",
                "DispatchGizmo::set_charges",
            ]
        );
        let add = registry.get("DispatchGizmo::add_charges").unwrap();
        assert_eq!(add.sig, Signature::new([Param::new(gizmo()), Param::new(Type::Int)], Type::Int));
        assert!(!add.flags.side_effect_free);
    }

    #[test]
    fn scripts_read_write_and_call_world_components() {
        let registry = NativeRegistry::with_engine_natives();
        let mut module = Module::new("gizmo_user");
        let import = |name: &str, params: Vec<Param>, ret: Type| Import { name: name.into(), sig: Signature::new(params, ret) };
        module.imports = vec![
            import("DispatchGizmo::of", vec![Param::new(Type::Entity)], gizmo()),
            import("DispatchGizmo::add_charges", vec![Param::new(gizmo()), Param::new(Type::Int)], Type::Int),
            import("DispatchGizmo::set_charges", vec![Param::new(gizmo()), Param::new(Type::Int)], Type::Unit),
            import("DispatchGizmo::get_charges", vec![Param::new(gizmo())], Type::Int),
        ];
        // g = DispatchGizmo::of(self); g.add_charges(n); g.charges = g.charges * 2; g.charges
        module.functions = vec![Function {
            name: "run".into(),
            exported: true,
            params: vec![Type::Int],
            ret: Type::Int,
            registers: vec![Type::Int, Type::Entity, gizmo(), Type::Int],
            code: vec![
                Instr::SelfEntity { dst: 1 },
                Instr::CallNative { import: 0, args: vec![1], dst: Some(2) },
                Instr::CallNative { import: 1, args: vec![2, 0], dst: Some(3) },
                Instr::Binary { op: pulsar_script_vm::BinOp::Add, dst: 3, a: 3, b: 3 },
                Instr::CallNative { import: 2, args: vec![2, 3], dst: None },
                Instr::CallNative { import: 3, args: vec![2], dst: Some(3) },
                Instr::Return { value: Some(3) },
            ],
            debug: None,
        }];
        let program = Program::link(Arc::new(module), &registry).unwrap();
        let (mut world, e) = hydrated_world(1);
        let mut vm = Vm::new();
        let mut instance = program.instantiate();
        let func = program.entry("run").unwrap();
        let mut host = Host::new(&mut world, e);
        let out = vm
            .call(&program, &mut instance, func, &[Value::Int(4)], &mut host, &mut Budget::new(100))
            .unwrap();
        assert_eq!(out, Value::Int(10));
        assert_eq!(world.get::<DispatchGizmo>(e).unwrap().charges, 10);

        // A reference whose entity lost the component fails cleanly.
        world.remove::<DispatchGizmo>(e);
        let mut host = Host::new(&mut world, e);
        let err = vm
            .call(&program, &mut instance, func, &[Value::Int(1)], &mut host, &mut Budget::new(100))
            .unwrap_err();
        assert!(err.to_string().contains("has no DispatchGizmo"), "{err}");
    }
}

/// #888: `Entity::DANGLING` is scripts' `entity::none()` (an unmatched
/// lookup, an unbound instance's `self`), so method and property access on
/// it is an ordinary "not live" error in every build, debug included: no
/// assert, no panic.
#[test]
fn dangling_entity_is_not_live_not_a_panic() {
    let (mut world, _e) = hydrated_world(3);
    let none = Entity::DANGLING;

    let err = invoke_component_method(
        &mut world,
        none,
        "DispatchGizmo",
        0,
        "add_charges",
        vec![Box::new(1i32)],
    )
    .unwrap_err();
    assert_eq!(err, ScriptRefError::despawned(none));
    assert!(get_component_property(&world, none, "DispatchGizmo", 0, "charges").is_err());
}

/// Manual release-mode baseline for the current property read paths.
///
/// Run with:
/// `cargo test -p pulsar_world_registry --test component_dispatch --release baseline_property_reads -- --ignored --nocapture`
///
/// This intentionally has no pass/fail latency threshold. It records local
/// before-change numbers for the direct typed field read, boxed reflected
/// dispatch, and JSON reflected dispatch. The latter two resolve
/// metadata through the shared descriptor cache (no EngineClass is
/// constructed per access; see `property_access_builds_descriptors_once`).
#[test]
#[ignore = "manual release-mode property access baseline"]
fn baseline_property_reads() {
    use std::hint::black_box;
    use std::time::Instant;

    const ITERATIONS: usize = 300_000;
    let (world, entity) = hydrated_world(42);

    let measure = |name: &str, mut access: Box<dyn FnMut() -> i32>| {
        ALLOCATION_CALLS.store(0, std::sync::atomic::Ordering::Relaxed);
        let started = Instant::now();
        let mut checksum = 0i64;
        for _ in 0..ITERATIONS {
            checksum += i64::from(black_box(access()));
        }
        let elapsed = started.elapsed();
        let allocations = ALLOCATION_CALLS.load(std::sync::atomic::Ordering::Relaxed);
        println!(
            "{name}: {:.1} ns/access, {:.3} allocation calls/access over {ITERATIONS} iterations (checksum {checksum})",
            elapsed.as_nanos() as f64 / ITERATIONS as f64,
            allocations as f64 / ITERATIONS as f64,
        );
    };

    measure(
        "direct typed SceneDB read",
        Box::new(|| black_box(world.get::<DispatchGizmo>(entity).unwrap().charges)),
    );
    measure(
        "boxed reflected read",
        Box::new(|| {
            *get_component_property_boxed(&world, entity, "DispatchGizmo", 0, "charges")
                .unwrap()
                .downcast::<i32>()
                .unwrap()
        }),
    );
    measure(
        "JSON reflected read",
        Box::new(|| {
            get_component_property(&world, entity, "DispatchGizmo", 0, "charges")
                .unwrap()
                .as_i64()
                .unwrap() as i32
        }),
    );
}

/// #886: reading and writing a property any number of times builds the
/// class's property table at most once, not once per access.
#[test]
fn property_access_builds_descriptors_once() {
    let (mut world, entity) = hydrated_world(1);
    // The first access may build the (process-wide, cached) table.
    let _ = get_component_property_boxed(&world, entity, "DispatchGizmo", 0, "charges").unwrap();
    let before = PROPERTY_TABLE_BUILDS.with(|n| n.get());
    for i in 0..100 {
        get_component_property_boxed(&world, entity, "DispatchGizmo", 0, "charges").unwrap();
        get_component_property(&world, entity, "DispatchGizmo", 0, "charges").unwrap();
        set_component_property_boxed(&mut world, entity, "DispatchGizmo", 0, "charges", Box::new(i as i32)).unwrap();
        set_component_property(&mut world, entity, "DispatchGizmo", 0, "charges", serde_json::json!(i)).unwrap();
    }
    assert_eq!(PROPERTY_TABLE_BUILDS.with(|n| n.get()), before, "a throwaway instance was built per access");
    assert_eq!(world.get::<DispatchGizmo>(entity).unwrap().charges, 99);

    // Unknown names still fail with the typed error, and are not cached as hits.
    assert!(matches!(
        get_component_property_boxed(&world, entity, "DispatchGizmo", 0, "nope"),
        Err(ScriptRefError::UnknownProperty { .. })
    ));
}
