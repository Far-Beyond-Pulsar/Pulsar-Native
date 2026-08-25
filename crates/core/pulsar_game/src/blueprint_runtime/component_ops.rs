//! Component-op execution bridge for the bytecode VM.
//!
//! The VM calls component operations (`comp_*::Class::Member`, arena ABI in
//! `pbgc::bytecode::comp_ops`) through plain `DispatchFn` pointers that
//! carry only arena addresses — no world, no entity. This module closes
//! that gap with a thread-local execution context:
//!
//! 1. [`run_with_component_context`] installs `{&mut World, Option<Entity>}`
//!    for the duration of one event execution.
//! 2. Blueprint preparation patches comp-op calls to the three trampolines
//!    below ([`component_op_handlers()`]).
//! 3. Each trampoline parses the staged operands and routes through
//!    `pulsar_world_registry`'s reflection dispatcher — the same single
//!    dispatch path native scripts and generated actors use.
//!
//! # Invariants
//!
//! * The context holds a raw `*mut World`. It is sound only because
//!   installation happens inside `run_with_component_context`, where the
//!   caller provably holds `&mut World` for the whole closure, and nested
//!   installation panics. One thread executes one blueprint event at a
//!   time.
//! * Trampolines are `extern "C"` — they must not unwind. Every failure
//!   (no context, unbound instance, despawned entity, unknown class,
//!   bad arguments) is logged and degrades to a null output rather than
//!   aborting. Graph-visible failure outputs remain future work; the
//!   log carries the typed `ScriptRefError` display.
//! * An instance that is registered but not yet bound to a scene entity
//!   (`entity: None` in the context, #648's binding model) skips component
//!   ops with an error log; graphs without component nodes are unaffected.

use pbgc::bytecode::comp_ops::{json_blob_len, write_json_blob};
use pulsar_bp_executor::{ComponentOpHandlers, CompOpKind};
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::dispatch::{
    get_component_property, invoke_component_method, json_args_to_method_args,
    set_component_property,
};
use pulsar_world_registry::marshal::any_to_json;
use serde_json::Value as JsonValue;
use std::cell::RefCell;

/// Handler addresses handed to `BpExecutor::prepare_with_component_ops`.
///
/// A function rather than a `const`: function-pointer-to-usize casts are
/// rejected by const evaluation.
pub fn component_op_handlers() -> ComponentOpHandlers {
    ComponentOpHandlers {
        get: comp_op_get_trampoline as *const () as usize as u64,
        set: comp_op_set_trampoline as *const () as usize as u64,
        call: comp_op_call_trampoline as *const () as usize as u64,
    }
}

/// The world slice one blueprint event executes against.
struct CompExecContext {
    world: *mut World,
    entity: Option<Entity>,
}

thread_local! {
    static COMP_EXEC_CTX: RefCell<Option<CompExecContext>> = const { RefCell::new(None) };
}

/// RAII guard clearing the thread-local context on scope exit, including
/// unwinds from graph logic running inside the VM loop.
struct CompExecGuard;

impl Drop for CompExecGuard {
    fn drop(&mut self) {
        COMP_EXEC_CTX.with(|slot| {
            slot.borrow_mut().take();
        });
    }
}

/// Install the context and run `f`.
///
/// Panics on nested installation: re-entry would alias two `&mut World`
/// borrows behind one context slot.
pub fn run_with_component_context<R>(
    world: &mut World,
    entity: Option<Entity>,
    f: impl FnOnce() -> R,
) -> R {
    COMP_EXEC_CTX.with(|slot| {
        let mut current = slot.borrow_mut();
        assert!(
            current.is_none(),
            "nested blueprint component contexts are not supported"
        );
        *current = Some(CompExecContext { world, entity });
        drop(current);
        let _guard = CompExecGuard;
        f()
    })
}

/// Run `f` with the installed context's world/entity.
///
/// Returns `None` (after logging) when no context is installed or the
/// running instance is not yet bound to a scene object.
fn with_context<R>(
    op: CompOpKind,
    class_name: &str,
    member: &str,
    f: impl FnOnce(&mut World, Entity) -> R,
) -> Option<R> {
    COMP_EXEC_CTX.with(|slot| {
        let current = slot.borrow_mut();
        let Some(ctx) = current.as_ref() else {
            tracing::error!(
                "blueprint {op:?}::{class_name}::{member} ran without a component \
                 context — program was not prepared with component handlers"
            );
            return None;
        };
        let Some(entity) = ctx.entity else {
            tracing::error!(
                "blueprint {op:?}::{class_name}::{member} ran on an instance not \
                 yet bound to a scene object"
            );
            return None;
        };
        // SAFETY: installed by `run_with_component_context`, whose caller
        // held `&mut World` for the whole closure body we are inside of.
        Some(f(unsafe { &mut *ctx.world }, entity))
    })
}

/// Read a staged NUL-separated name blob from the arena.
///
/// Get/set stage `{class}\0{member}\0`; calls stage
/// `{class}\0{member}\0{arg_count}\0`.
///
/// # Safety
/// `ptr` must point into the live arena at a blob written by the codegen.
unsafe fn read_name_fields(ptr: *const u8, fields: usize) -> Option<Vec<String>> {
    let mut end = ptr;
    let mut terminators = 0usize;
    while terminators < fields {
        if *end == 0 {
            terminators += 1;
        }
        end = end.add(1);
    }
    let bytes = std::slice::from_raw_parts(ptr, end.offset_from(ptr) as usize);
    let text = std::str::from_utf8(bytes).ok()?;
    let parts: Vec<String> = text.strip_suffix('\0')?.split('\0').map(str::to_string).collect();
    if parts.len() != fields {
        return None;
    }
    Some(parts)
}

/// Read a length-prefixed JSON value blob from the arena.
///
/// # Safety
/// `ptr` must point at a blob written by the codegen or a prior trampoline.
unsafe fn read_json_value(ptr: *const u8) -> Option<JsonValue> {
    let len = json_blob_len(ptr);
    let bytes = std::slice::from_raw_parts(ptr.add(8), len);
    serde_json::from_slice(bytes).ok()
}

/// Log a dispatcher failure on behalf of one graph node.
fn log_failure(op: CompOpKind, class_name: &str, member: &str, error: impl std::fmt::Display) {
    tracing::error!("blueprint {op:?}::{class_name}::{member} failed: {error}");
}

/// Trampoline for `comp_get_prop::Class::Prop` (pure producer).
///
/// Writes the property's JSON into the reserved output blob; leaves the
/// slot zeroed (decodes as JSON `null`) if anything fails.
pub unsafe extern "C" fn comp_op_get_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let Some(fields) = read_name_fields(*args, 2) else { return };
    let (class_name, prop_name) = (&fields[0], &fields[1]);
    let Some(result) = with_context(CompOpKind::GetProp, class_name, prop_name, |world, entity| {
        get_component_property(world, entity, class_name, 0, prop_name)
    }) else {
        return;
    };
    match result {
        Ok(value) => write_json_blob(ret, &value.to_string()),
        Err(error) => log_failure(CompOpKind::GetProp, class_name, prop_name, error),
    }
}

/// Trampoline for `comp_set_prop::Class::Prop` (exec consumer).
pub unsafe extern "C" fn comp_op_set_trampoline(
    args: *const *const u8,
    _ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let Some(fields) = read_name_fields(*args, 2) else { return };
    let (class_name, prop_name) = (&fields[0], &fields[1]);
    let Some(value) = read_json_value(*args.add(1)) else {
        tracing::error!("blueprint set::{class_name}::{prop_name}: value operand is not valid JSON");
        return;
    };
    if let Some(Err(error)) = with_context(CompOpKind::SetProp, class_name, prop_name, |world, entity| {
        set_component_property(world, entity, class_name, 0, prop_name, value)
    }) {
        log_failure(CompOpKind::SetProp, class_name, prop_name, error);
    }
}

/// Trampoline for `comp_call::Class::Method` (exec, optional return).
///
/// Arguments arrive as JSON blobs in pin order and are converted to their
/// declared types via reflection metadata before dispatch; the return value
/// (if the graph uses one) is written back as a JSON blob.
pub unsafe extern "C" fn comp_op_call_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let Some(fields) = read_name_fields(*args, 3) else { return };
    let (class_name, method_name) = (&fields[0].clone(), &fields[1].clone());
    let arg_count: usize = match fields[2].parse() {
        Ok(count) => count,
        Err(_) => {
            tracing::error!(
                "blueprint call::{class_name}::{method_name}: malformed argument count {:?}",
                fields[2]
            );
            return;
        }
    };

    let mut arg_values = Vec::with_capacity(arg_count);
    for index in 0..arg_count {
        match read_json_value(*args.add(1 + index)) {
            Some(value) => arg_values.push(value),
            None => {
                tracing::error!(
                    "blueprint call::{class_name}::{method_name}: argument {index} is not valid JSON"
                );
                return;
            }
        }
    }

    // Convert JSON arguments to their declared types through the shared
    // dispatcher helper (#643's guarantee, one policy for both adapters —
    // the generated Rust actors call the exact same function).
    let method_args = match json_args_to_method_args(class_name, method_name, arg_values) {
        Ok(args) => args,
        Err(error) => {
            log_failure(CompOpKind::Call, class_name, method_name, error);
            return;
        }
    };

    let outcome = with_context(CompOpKind::Call, class_name, method_name, |world, entity| {
        invoke_component_method(world, entity, class_name, 0, method_name, method_args)
    });
    let Some(returned) = outcome else { return };
    match returned {
        Ok(value) => {
            if !ret.is_null() {
                write_return(class_name, method_name, value, ret);
            }
        }
        Err(error) => log_failure(CompOpKind::Call, class_name, method_name, error),
    }
}

/// Serialize a method's return value into the graph-visible output blob.
///
/// Serialization runs off the value's own concrete type
/// (`RUNTIME_TYPE_REGISTRY.serialize_json_for_any`); no declared return type
/// is required for a `Pure`/`Fn` method to hand a result back to the graph.
unsafe fn write_return(
    class_name: &str,
    method_name: &str,
    returned: Option<Box<dyn std::any::Any>>,
    ret: *mut u8,
) {
    let Some(returned) = returned else {
        write_json_blob(ret, "null");
        return;
    };
    match any_to_json("blueprint call return", returned.as_ref()) {
        Ok(json) => write_json_blob(ret, &json.to_string()),
        Err(error) => log_failure(CompOpKind::Call, class_name, method_name, error),
    }
}

#[cfg(test)]
mod tests {
    //! Executes compiled component-op programs against a real `World`,
    //! proving the trampoline -> reflection-dispatcher path end to end
    //! (#647). The probe component registers exactly like engine classes
    //! do (`#[engine_class]` expands to the same inventory submissions).

    use super::*;
    use pbgc::bytecode::comp_ops::{
        encode_call_name_blob, encode_json_blob, encode_name_blob, JSON_BLOB_CAPACITY,
    };
    use pbgc::{BpProgram, Instruction};
    use pulsar_reflection::{
        ComponentMethodRegistration, EngineClass, EngineClassRegistration, MethodMetadata,
        MethodReturnType, MethodParameter, MethodType, PropertyMetadata, RuntimeTypeInfo,
        RUNTIME_TYPE_REGISTRY,
    };
    use pulsar_scenedb::component_id;
    use pulsar_world_registry::WorldComponentRegistration;
    use serde::{Deserialize, Serialize};

    // ── Probe component ──────────────────────────────────────────────────

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
                params: vec![MethodParameter { name: "amount", type_info: i32_info }],
                return_type: Some(MethodReturnType { type_info: i32_info }),
                method_type: MethodType::Fn,
                caller: Box::new(|c: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                    let amount = args.first().and_then(|a| a.downcast_ref::<i32>()).copied()?;
                    let probe = c.as_any_mut().downcast_mut::<VmProbe>()?;
                    probe.charges += amount;
                    Some(Box::new(probe.charges))
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

        fn to_json(&self) -> Result<JsonValue, String> {
            serde_json::to_value(self).map_err(|e| e.to_string())
        }
    }

    fn vm_probe_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
        world.get::<VmProbe>(entity).map(|c| c as &dyn EngineClass)
    }

    fn vm_probe_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
        world.get_mut::<VmProbe>(entity).map(|c| c.into_inner() as &mut dyn EngineClass)
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

    // ── Program construction helpers ─────────────────────────────────────

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
            if let Instruction::Call { node_type, fn_ptr, .. } = instr {
                *fn_ptr = match parse_node_kind(node_type) {
                    Some(CompOpKind::GetProp) => component_op_handlers().get,
                    Some(CompOpKind::SetProp) => component_op_handlers().set,
                    Some(CompOpKind::Call) => component_op_handlers().call,
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
        program.instructions.insert(0, Instruction::InitBytes { offset, bytes });
        offset
    }

    #[test]
    fn vm_set_writes_live_world_property() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, VmProbe { charges: 1 });

        let mut prog = build_program(vec![]);
        let name = stage(&mut prog, encode_name_blob("VmProbe", "charges"));
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

        run_with_component_context(&mut world, Some(entity), || {
            pbgc::vm::run(&prog).unwrap();
        });

        assert_eq!(world.get::<VmProbe>(entity).unwrap().charges, 7);
    }

       #[test]
    fn vm_get_reads_live_world_property_into_output_blob() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, VmProbe { charges: 123 });

        let mut prog = build_program(vec![]);
        let name = stage(&mut prog, encode_name_blob("VmProbe", "charges"));
        let output_offset = prog.arena_size;
        prog.arena_size += JSON_BLOB_CAPACITY + 8;
        prog.instructions.push(Instruction::Call {
            fn_ptr: 0,
            node_type: "comp_get_prop::VmProbe::charges".into(),
            input_offsets: vec![name],
            output_offset,
            has_output: true,
            type_slot_offsets: vec![],
        });
        bind_handlers(&mut prog);

        let mut arena = vec![0u64; prog.arena_size.div_ceil(8)];
        let base = arena.as_mut_ptr() as *mut u8;
        run_with_component_context(&mut world, Some(entity), || {
            unsafe {
                pbgc::vm::run_with_external_arena(&prog, base, prog.arena_size).unwrap();
            }
        });

        let len = unsafe { json_blob_len(base.add(output_offset)) };
        let bytes = unsafe { std::slice::from_raw_parts(base.add(output_offset + 8), len) };
        assert_eq!(std::str::from_utf8(bytes).unwrap(), "123");
    }
 #[test]
    fn vm_call_dispatches_through_reflection() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, VmProbe { charges: 40 });

        let mut prog = build_program(vec![]);
        let name = stage(&mut prog, encode_call_name_blob("VmProbe", "add_charges", 1));
        let arg = stage(&mut prog, encode_json_blob("2"));
        let output_offset = prog.arena_size;
        prog.arena_size += JSON_BLOB_CAPACITY;
        prog.instructions.push(Instruction::Call {
            fn_ptr: 0,
            node_type: "comp_call::VmProbe::add_charges".into(),
            input_offsets: vec![name, arg],
            output_offset,
            has_output: true,
            type_slot_offsets: vec![],
        });
        bind_handlers(&mut prog);

        run_with_component_context(&mut world, Some(entity), || {
            pbgc::vm::run(&prog).unwrap();
        });

        assert_eq!(world.get::<VmProbe>(entity).unwrap().charges, 42);
    }

    #[test]
    fn unbound_instance_ops_refuse_without_panicking() {
        let mut world = World::new();
        let entity = world.spawn();
        world.insert(entity, VmProbe { charges: 5 });

        let mut prog = build_program(vec![]);
        let name = stage(&mut prog, encode_name_blob("VmProbe", "charges"));
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
}