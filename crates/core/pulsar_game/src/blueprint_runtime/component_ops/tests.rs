//! Executes compiled component-op programs against a real `World`,
//! proving the trampoline -> reflection-dispatcher path end to end
//! (#647), plus the cross-object reference ops (#654): resolvers,
//! reference producers, and pin-targeted writes. The probe component
//! registers exactly like engine classes do (`#[engine_class]` expands
//! to the same inventory submissions).

use super::{component_op_handlers, run_with_component_context};
use pbgc::bytecode::comp_ops::{
    encode_json_blob, encode_targeted_call_name_blob, encode_targeted_name_blob, json_blob_len,
    RefTarget, JSON_BLOB_CAPACITY,
};
use pbgc::{BpProgram, Instruction};
use pulsar_bp_executor::CompOpKind;
use pulsar_reflection::{
    ComponentMethodRegistration, EngineClass, EngineClassRegistration, MethodMetadata,
    MethodParameter, MethodReturnType, MethodType, PropertyMetadata, RuntimeTypeInfo,
    RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{component_id, Entity, World};
use pulsar_world_registry::WorldComponentRegistration;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ── Probe component ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct VmProbe {
    charges: i32,
}

impl EngineClass for VmProbe {
    fn class_name() -> &'static str {
        "VmProbe"
    }

    fn get_properties(&self) -> Vec<PropertyMetadata> {
        let type_info: &'static RuntimeTypeInfo =
            RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 registered");
        vec![PropertyMetadata {
            name: "charges",
            display_name: "Charges".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info,
            getter: Box::new(|c: &dyn EngineClass| {
                Box::new(c.as_any().downcast_ref::<VmProbe>().unwrap().charges)
            }),
            setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                if let Some(v) = v.downcast_ref::<i32>() {
                    c.as_any_mut().downcast_mut::<VmProbe>().unwrap().charges = *v;
                }
            }),
        }]
    }

    fn get_methods() -> Vec<MethodMetadata> {
        let i32_info: &'static RuntimeTypeInfo =
            RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 registered");
        vec![MethodMetadata {
            name: "add_charges",
            display_name: "Add Charges".into(),
            category: None,
            params: vec![MethodParameter {
                name: "amount",
                type_info: i32_info,
            }],
            return_type: Some(MethodReturnType {
                type_info: i32_info,
            }),
            method_type: MethodType::Fn,
            caller: Box::new(
                |c: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                    let amount = args
                        .first()
                        .and_then(|a| a.downcast_ref::<i32>())
                        .copied()?;
                    let probe = c.as_any_mut().downcast_mut::<VmProbe>()?;
                    probe.charges += amount;
                    Some(Box::new(probe.charges))
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

    fn to_json(&self) -> Result<JsonValue, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}

fn vm_probe_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
    world.get::<VmProbe>(entity).map(|c| c as &dyn EngineClass)
}

fn vm_probe_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
    world
        .get_mut::<VmProbe>(entity)
        .map(|c| c.into_inner() as &mut dyn EngineClass)
}

fn vm_probe_hydrate(world: &mut World, entity: Entity, data: &JsonValue) -> Result<(), String> {
    let parsed: VmProbe = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    world.insert(entity, parsed);
    Ok(())
}

fn vm_probe_remove(world: &mut World, entity: Entity) {
    let _ = world.remove::<VmProbe>(entity);
}

pulsar_world_registry::inventory::submit! {
    WorldComponentRegistration {
        class_name: "VmProbe",
        component_type: component_id::<VmProbe>,
        hydrate: vm_probe_hydrate,
        remove: vm_probe_remove,
        dispatch: |world, entity, _: _, _: usize, _: _| world.get::<VmProbe>(entity).is_some(),
        get_as_engine_class: vm_probe_get,
        get_as_engine_class_mut: vm_probe_get_mut,
        on_removed: |_, _| {},
        refresh_gpu_mirror: |_, _| {},
    }
}

pulsar_reflection::inventory::submit! {
    EngineClassRegistration {
        name: "VmProbe",
        category: None,
        constructor: <VmProbe as EngineClass>::create_default,
        from_json: None,
    }
}

pulsar_reflection::inventory::submit! {
    ComponentMethodRegistration {
        class_name: "VmProbe",
        methods: <VmProbe as EngineClass>::get_methods,
    }
}

// ── Program construction helpers ─────────────────────────────────────────

/// Minimal begin_play program containing one staged component op.
fn build_program(ops: Vec<pbgc::Instruction>) -> BpProgram {
    let mut prog = BpProgram::new("begin_play");
    prog.instructions = ops;
    prog.max_args_count = 4;
    prog.arena_size = 64 * 1024;
    prog
}

fn bind_handlers(program: &mut BpProgram) {
    for instr in &mut program.instructions {
        if let Instruction::Call {
            node_type, fn_ptr, ..
        } = instr
        {
            *fn_ptr = match parse_node_kind(node_type) {
                Some(CompOpKind::GetProp) => component_op_handlers().get,
                Some(CompOpKind::SetProp) => component_op_handlers().set,
                Some(CompOpKind::Call) => component_op_handlers().call,
                Some(CompOpKind::GetRef) => component_op_handlers().get_ref,
                Some(CompOpKind::FindByStableId) => component_op_handlers().find_by_stable_id,
                Some(CompOpKind::FindByName) => component_op_handlers().find_by_name,
                Some(CompOpKind::ObjectLiteral) => component_op_handlers().object_literal,
                None => panic!("unexpected node {}", node_type),
            };
        }
    }
}

fn parse_node_kind(node_type: &str) -> Option<CompOpKind> {
    pulsar_bp_executor::parse_node_type(node_type).map(|(kind, _, _)| kind)
}

fn stage(program: &mut BpProgram, bytes: Vec<u8>) -> usize {
    let offset = program.arena_size;
    program.arena_size += bytes.len() + 8;
    program
        .instructions
        .insert(0, Instruction::InitBytes { offset, bytes });
    offset
}

fn reserve_output(program: &mut BpProgram) -> usize {
    let offset = program.arena_size;
    program.arena_size += JSON_BLOB_CAPACITY + 8;
    offset
}

/// Run `program` against `world` as `entity`'s instance.
fn run_in_world(program: &BpProgram, world: &mut World, entity: Option<Entity>) {
    run_with_component_context(world, entity, || {
        pbgc::vm::run(program).unwrap();
    });
}

/// Read back a JSON blob written into the arena at `offset`.
fn read_output(arena: &[u8], offset: usize) -> JsonValue {
    let base = arena.as_ptr();
    let len = unsafe { json_blob_len(base.add(offset)) };
    let bytes = &arena[offset + 8..offset + 8 + len];
    serde_json::from_slice(bytes).expect("output decodes")
}

/// Execute `program` with an externally owned arena and return it.
fn run_collecting_output(program: &BpProgram, world: &mut World, entity: Entity) -> Vec<u8> {
    let mut arena = vec![0u8; program.arena_size];
    let base = arena.as_mut_ptr();
    run_with_component_context(world, Some(entity), || {
        // SAFETY: arena outlives the call; size matches.
        unsafe { pbgc::vm::run_with_external_arena(program, base, program.arena_size).unwrap() };
    });
    arena
}

// ── Self-targeted op regression (#647/#D2) ───────────────────────────────

#[test]
fn vm_set_writes_live_world_property() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, VmProbe { charges: 1 });

    let mut prog = build_program(vec![]);
    let name = stage(
        &mut prog,
        encode_targeted_name_blob(
            "VmProbe",
            "charges",
            pbgc::bytecode::comp_ops::RefTarget::SelfActor,
        ),
    );
    let value = stage(&mut prog, encode_json_blob("7"));
    prog.instructions.push(Instruction::Call {
        fn_ptr: 0,
        node_type: "comp_set_prop::VmProbe::charges".into(),
        input_offsets: vec![name, value],
        output_offset: 0,
        has_output: false,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut prog);

    run_in_world(&prog, &mut world, Some(entity));

    assert_eq!(world.get::<VmProbe>(entity).unwrap().charges, 7);
}

#[test]
fn vm_get_reads_live_world_property_into_output_blob() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, VmProbe { charges: 123 });

    let mut prog = build_program(vec![]);
    let name = stage(
        &mut prog,
        encode_targeted_name_blob(
            "VmProbe",
            "charges",
            pbgc::bytecode::comp_ops::RefTarget::SelfActor,
        ),
    );
    let output_offset = reserve_output(&mut prog);
    prog.instructions.push(Instruction::Call {
        fn_ptr: 0,
        node_type: "comp_get_prop::VmProbe::charges".into(),
        input_offsets: vec![name],
        output_offset,
        has_output: true,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut prog);

    let arena = run_collecting_output(&prog, &mut world, entity);
    assert_eq!(read_output(&arena, output_offset), serde_json::json!(123));
}

#[test]
fn vm_call_dispatches_through_reflection() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, VmProbe { charges: 40 });

    let mut prog = build_program(vec![]);
    let name = stage(
        &mut prog,
        encode_targeted_call_name_blob(
            "VmProbe",
            "add_charges",
            1,
            pbgc::bytecode::comp_ops::RefTarget::SelfActor,
        ),
    );
    let arg = stage(&mut prog, encode_json_blob("2"));
    let output_offset = reserve_output(&mut prog);
    prog.instructions.push(Instruction::Call {
        fn_ptr: 0,
        node_type: "comp_call::VmProbe::add_charges".into(),
        input_offsets: vec![name, arg],
        output_offset,
        has_output: true,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut prog);

    run_in_world(&prog, &mut world, Some(entity));

    assert_eq!(world.get::<VmProbe>(entity).unwrap().charges, 42);
}

#[test]
fn unbound_instance_ops_refuse_without_panicking() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, VmProbe { charges: 5 });

    let mut prog = build_program(vec![]);
    let name = stage(
        &mut prog,
        encode_targeted_name_blob(
            "VmProbe",
            "charges",
            pbgc::bytecode::comp_ops::RefTarget::SelfActor,
        ),
    );
    let value = stage(&mut prog, encode_json_blob("999"));
    prog.instructions.push(Instruction::Call {
        fn_ptr: 0,
        node_type: "comp_set_prop::VmProbe::charges".into(),
        input_offsets: vec![name, value],
        output_offset: 0,
        has_output: false,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut prog);

    // No context installed at all — must log and return, not abort.
    pbgc::vm::run(&prog).unwrap();
    assert_eq!(world.get::<VmProbe>(entity).unwrap().charges, 5);
}

// ── Cross-object references (#654) ───────────────────────────────────────

/// A two-entity scene shaped like an editor-hydrated level: StableId/Name
/// components attached exactly like `WorldSceneStore::spawn` does, plus
/// probe components on both objects so self-vs-target writes are visible.
fn scene() -> (World, Entity, Entity) {
    let mut world = World::new();
    let trigger = world.spawn();
    world.insert(trigger, engine_backend::scene::StableId("trigger".into()));
    world.insert(trigger, engine_backend::scene::Name("Trigger".into()));
    world.insert(trigger, VmProbe { charges: 1 });
    let lamp = world.spawn();
    world.insert(lamp, engine_backend::scene::StableId("lamp".into()));
    world.insert(lamp, engine_backend::scene::Name("Red Lamp".into()));
    world.insert(lamp, VmProbe { charges: 2 });
    (world, trigger, lamp)
}

/// THE #654 acceptance chain, executed through the real VM path:
/// find_object_by_name -> get_component_ref(on that actor) -> comp_set_prop
/// (pin-targeted). The write must land on the FOUND entity's component,
/// never on the executing instance's.
#[test]
fn cross_object_chain_resolves_and_writes_the_found_entity() {
    let (mut world, trigger, lamp) = scene();
    let mut prog = build_program(vec![]);

    // find_object_by_name("Red Lamp") -> ActorRef blob.
    let find_needle = stage(&mut prog, encode_json_blob("\"Red Lamp\""));
    let find_output = reserve_output(&mut prog);
    prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().find_by_name,
        node_type: "find_object_by_name".into(),
        input_offsets: vec![find_needle],
        output_offset: find_output,
        has_output: true,
        type_slot_offsets: vec![],
    });

    // get_component_ref::VmProbe::0 with that actor operand.
    let get_ref_name = stage(
        &mut prog,
        encode_targeted_name_blob("VmProbe", "0", RefTarget::RefPin),
    );
    let get_ref_output = reserve_output(&mut prog);
    prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().get_ref,
        node_type: "get_component_ref::VmProbe::0".into(),
        input_offsets: vec![get_ref_name, find_output],
        output_offset: get_ref_output,
        has_output: true,
        type_slot_offsets: vec![],
    });

    // comp_set_prop THROUGH the produced reference (pin-targeted shape).
    let set_name = stage(
        &mut prog,
        encode_targeted_name_blob("VmProbe", "charges", RefTarget::RefPin),
    );
    let value = stage(&mut prog, encode_json_blob("99"));
    prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().set,
        node_type: "comp_set_prop::VmProbe::charges".into(),
        input_offsets: vec![set_name, get_ref_output, value],
        output_offset: 0,
        has_output: false,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut prog);

    run_in_world(&prog, &mut world, Some(trigger));

    assert_eq!(
        world.get::<VmProbe>(lamp).unwrap().charges,
        99,
        "the found entity's component must be written"
    );
    assert_eq!(
        world.get::<VmProbe>(trigger).unwrap().charges,
        1,
        "the executing instance must stay untouched"
    );
}

/// #654: an object_ref_literal resolves its stable id at RUNTIME; the
/// staged operand is the save/load form, never entity bits.
#[test]
fn object_literal_resolves_its_stable_id_against_the_live_world() {
    let (mut world, _trigger, lamp) = scene();

    let mut prog = build_program(vec![]);
    let literal_operand = stage(
        &mut prog,
        encode_json_blob(
            "{\"stable_id\":\"lamp\",\"class_name\":\"VmProbe\",\"component_index\":0}",
        ),
    );
    let literal_output = reserve_output(&mut prog);
    prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().object_literal,
        node_type: "object_ref_literal".into(),
        input_offsets: vec![literal_operand],
        output_offset: literal_output,
        has_output: true,
        type_slot_offsets: vec![],
    });

    // Write THROUGH the resolved reference to prove it addresses `lamp`.
    let set_name = stage(
        &mut prog,
        encode_targeted_name_blob("VmProbe", "charges", RefTarget::RefPin),
    );
    let value = stage(&mut prog, encode_json_blob("7"));
    prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().set,
        node_type: "comp_set_prop::VmProbe::charges".into(),
        input_offsets: vec![set_name, literal_output, value],
        output_offset: 0,
        has_output: false,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut prog);

    run_in_world(&prog, &mut world, Some(_trigger));

    assert_eq!(world.get::<VmProbe>(lamp).unwrap().charges, 7);
}

/// #654/#641: a stale or class-mismatched pin target degrades to a typed
/// logged failure — no write anywhere, no panic.
#[test]
fn pin_targets_fail_typed_without_misaddressing() {
    let (mut world, trigger, lamp) = scene();

    // Reference to a DESPAWNED actor.
    let mut stale_prog = build_program(vec![]);
    let stale_ref = stage(
        &mut stale_prog,
        encode_json_blob("{\"entity\":999999,\"class_name\":\"VmProbe\",\"component_index\":0}"),
    );
    let set_name = stage(
        &mut stale_prog,
        encode_targeted_name_blob("VmProbe", "charges", RefTarget::RefPin),
    );
    let value = stage(&mut stale_prog, encode_json_blob("50"));
    stale_prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().set,
        node_type: "comp_set_prop::VmProbe::charges".into(),
        input_offsets: vec![set_name, stale_ref, value],
        output_offset: 0,
        has_output: false,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut stale_prog);

    run_in_world(&stale_prog, &mut world, Some(trigger));
    assert_eq!(
        world.get::<VmProbe>(trigger).unwrap().charges,
        1,
        "self untouched by failed cross-object write"
    );

    // Class mismatch: a "Door" ref feeding a VmProbe op refuses (#519).
    let mut mismatch_prog = build_program(vec![]);
    let door_ref = stage(
        &mut mismatch_prog,
        encode_json_blob("{\"entity\":1,\"class_name\":\"Door\",\"component_index\":0}"),
    );
    let m_set_name = stage(
        &mut mismatch_prog,
        encode_targeted_name_blob("VmProbe", "charges", RefTarget::RefPin),
    );
    let m_value = stage(&mut mismatch_prog, encode_json_blob("51"));
    mismatch_prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().set,
        node_type: "comp_set_prop::VmProbe::charges".into(),
        input_offsets: vec![m_set_name, door_ref, m_value],
        output_offset: 0,
        has_output: false,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut mismatch_prog);

    run_in_world(&mismatch_prog, &mut world, Some(trigger));
    assert_eq!(world.get::<VmProbe>(trigger).unwrap().charges, 1);
    assert_eq!(world.get::<VmProbe>(lamp).unwrap().charges, 2);

    // Resolver misses degrade to null without panicking.
    let mut miss_prog = build_program(vec![]);
    let ghost_needle = stage(&mut miss_prog, encode_json_blob("\"Ghost\""));
    let ghost_output = reserve_output(&mut miss_prog);
    miss_prog.instructions.push(Instruction::Call {
        fn_ptr: component_op_handlers().find_by_name,
        node_type: "find_object_by_name".into(),
        input_offsets: vec![ghost_needle],
        output_offset: ghost_output,
        has_output: true,
        type_slot_offsets: vec![],
    });
    bind_handlers(&mut miss_prog);
    run_in_world(&miss_prog, &mut world, Some(trigger));
}
