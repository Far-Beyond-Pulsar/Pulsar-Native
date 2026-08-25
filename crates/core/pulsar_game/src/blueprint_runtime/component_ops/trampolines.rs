//! The seven `extern "C"` comp-op trampolines (ABI: `pbgc::bytecode::
//! comp_ops`). Each parses its staged operands — fixed field counts per
//! kind, so no length side-channel exists — and routes through the one
//! dispatch layer (`pulsar_world_registry::dispatch`) or the shared
//! reference-resolution helpers (`crate::script_refs`).
//!
//! All are `#[no_mangle]`-free internal addresses patched into programs by
//! [`super::component_op_handlers`]; none may unwind across the VM's call
//! boundary.

use pbgc::bytecode::comp_ops::{
    decode_targeted_call_name_blob, decode_targeted_name_blob, json_blob_len, write_json_blob,
    RefTarget,
};
use pulsar_bp_executor::CompOpKind;
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::dispatch::{
    get_component_property, invoke_component_method, json_args_to_method_args,
    set_component_property,
};
use pulsar_world_registry::marshal::any_to_json;
use serde_json::Value as JsonValue;

use super::{context_snapshot, with_context, with_world};

/// Resolve a pin-targeted op's `(entity, component_index)` from its
/// reference operand; self-targeted ops resolve to `(instance entity, 0)`
/// — the pre-#654 addressing. `None` after logging on any failure.
fn resolve_op_target(
    op: CompOpKind,
    class_name: &str,
    member: &str,
    world: &mut World,
    instance_entity: Entity,
    target: RefTarget,
    ref_operand: *const u8,
) -> Option<(Entity, u32)> {
    match target {
        RefTarget::SelfActor => Some((instance_entity, 0)),
        RefTarget::RefPin => {
            let reference = unsafe { read_json_value(ref_operand) }?;
            crate::script_refs::resolve_pin_target(
                world,
                &reference,
                class_name,
                &format!("{op:?}::{class_name}::{member}"),
            )
        }
    }
}

/// Read exactly `fields` NUL-terminated fields from the arena.
///
/// The ABI v2 codegen stages fixed field counts per kind (get/set/ref: 3,
/// call: 4), so scanning never runs past the blob into other operands.
///
/// # Safety
/// `ptr` must point into the live arena at a blob written by the codegen.
unsafe fn read_name_fields(ptr: *const u8, fields: usize) -> Option<Vec<u8>> {
    let mut end = ptr;
    let mut terminators = 0usize;
    while terminators < fields {
        if *end == 0 {
            terminators += 1;
        }
        end = end.add(1);
    }
    let bytes = std::slice::from_raw_parts(ptr, end.offset_from(ptr) as usize);
    Some(bytes.to_vec())
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
/// slot zeroed (decodes as JSON `null`) if anything fails. Operands:
/// `[name, (reference)]`.
pub unsafe extern "C" fn comp_op_get_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let blob = unsafe { read_name_fields(*args, 3) };
    let Some(blob) = blob else { return };
    let Some(fields) = decode_targeted_name_blob(&blob) else {
        tracing::error!("blueprint get prop name blob failed to decode");
        return;
    };
    let ref_operand = *args.add(1);
    let Some(outcome) = with_context(
        CompOpKind::GetProp,
        &fields.class_name,
        &fields.member,
        |world, instance_entity| {
            resolve_op_target(
                CompOpKind::GetProp,
                &fields.class_name,
                &fields.member,
                world,
                instance_entity,
                fields.target,
                ref_operand,
            )
            .and_then(|(entity, index)| {
                match get_component_property(world, entity, &fields.class_name, index, &fields.member)
                {
                    Ok(value) => Some(value),
                    Err(error) => {
                        log_failure(CompOpKind::GetProp, &fields.class_name, &fields.member, error);
                        None
                    }
                }
            })
        },
    ) else {
        return;
    };
    if let Some(value) = outcome {
        write_json_blob(ret, &value.to_string());
    }
}

/// Trampoline for `comp_set_prop::Class::Prop` (exec consumer).
///
/// Pin-targeted ops stage `[name, reference, value]`; self-targeted stage
/// `[name, value]`.
pub unsafe extern "C" fn comp_op_set_trampoline(
    args: *const *const u8,
    _ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let blob = unsafe { read_name_fields(*args, 3) };
    let Some(blob) = blob else { return };
    let Some(fields) = decode_targeted_name_blob(&blob) else {
        tracing::error!("blueprint set prop name blob failed to decode");
        return;
    };
    let value_slot = match fields.target {
        RefTarget::SelfActor => 1,
        RefTarget::RefPin => 2,
    };
    let value = match unsafe { read_json_value(*args.add(value_slot)) } {
        Some(value) => value,
        None => {
            tracing::error!(
                "blueprint set::{}::{}: value operand is not valid JSON",
                fields.class_name,
                fields.member
            );
            return;
        }
    };
    let ref_operand = *args.add(1);
    if let Some(Err(error)) = with_context(
        CompOpKind::SetProp,
        &fields.class_name,
        &fields.member,
        |world, instance_entity| {
            match resolve_op_target(
                CompOpKind::SetProp,
                &fields.class_name,
                &fields.member,
                world,
                instance_entity,
                fields.target,
                ref_operand,
            ) {
                Some((entity, index)) => {
                    set_component_property(
                        world, entity, &fields.class_name, index, &fields.member, value,
                    )
                }
                // Unresolved target was already logged by the resolver.
                None => Ok(()),
            }
        },
    ) {
        log_failure(CompOpKind::SetProp, &fields.class_name, &fields.member, error);
    }
}

/// Trampoline for `comp_call::Class::Method` (exec, optional return).
///
/// Arguments arrive as JSON blobs in pin order and are converted to their
/// declared types via reflection metadata before dispatch; the return value
/// (if the graph uses one) is written back as a JSON blob. Pin-targeted
/// calls stage `[name, reference, arg0..argN]`, self-targeted
/// `[name, arg0..argN]`.
pub unsafe extern "C" fn comp_op_call_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let blob = unsafe { read_name_fields(*args, 4) };
    let Some(blob) = blob else { return };
    let Some(fields) = decode_targeted_call_name_blob(&blob) else {
        tracing::error!("blueprint call name blob failed to decode");
        return;
    };
    let first_value = match fields.target {
        RefTarget::SelfActor => 1,
        RefTarget::RefPin => 2,
    };

    let mut arg_values = Vec::with_capacity(fields.arg_count);
    for index in 0..fields.arg_count {
        match unsafe { read_json_value(*args.add(first_value + index)) } {
            Some(value) => arg_values.push(value),
            None => {
                log_failure(
                    CompOpKind::Call,
                    &fields.class_name,
                    &fields.method,
                    format!("argument {index} is not valid JSON"),
                );
                return;
            }
        }
    }

    // Convert JSON arguments to their declared types through the shared
    // dispatcher helper (#643's guarantee, one policy for both adapters —
    // the generated Rust actors call the exact same function).
    let method_args =
        match json_args_to_method_args(&fields.class_name, &fields.method, arg_values) {
            Ok(args) => args,
            Err(error) => {
                log_failure(CompOpKind::Call, &fields.class_name, &fields.method, error);
                return;
            }
        };

    let ref_operand = *args.add(1);
    let outcome = with_context(
        CompOpKind::Call,
        &fields.class_name,
        &fields.method,
        |world, instance_entity| {
            match resolve_op_target(
                CompOpKind::Call,
                &fields.class_name,
                &fields.method,
                world,
                instance_entity,
                fields.target,
                ref_operand,
            ) {
                Some((entity, index)) => invoke_component_method(
                    world,
                    entity,
                    &fields.class_name,
                    index,
                    &fields.method,
                    method_args,
                ),
                None => Ok(None),
            }
        },
    );
    let Some(returned) = outcome else { return };
    match returned {
        Ok(value) => {
            if !ret.is_null() {
                write_return(&fields.class_name, &fields.method, value, ret);
            }
        }
        Err(error) => log_failure(CompOpKind::Call, &fields.class_name, &fields.method, error),
    }
}

/// Trampoline for `get_component_ref::Class::Index` (#654 pure producer).
///
/// Emits the referenced component's #642 JSON shape with live entity bits.
/// With an `actor` operand wired (pin-targeted), resolves the reference ON
/// that actor instead of the executing instance. Operands: `[name,
/// (actor)]`.
pub unsafe extern "C" fn comp_op_get_ref_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let blob = unsafe { read_name_fields(*args, 3) };
    let Some(blob) = blob else { return };
    let Some(fields) = decode_targeted_name_blob(&blob) else {
        tracing::error!("blueprint get_component_ref name blob failed to decode");
        return;
    };
    let Ok(component_index) = fields.member.parse::<u32>() else {
        tracing::error!(
            "blueprint get_component_ref::{}::{}: malformed component index",
            fields.class_name,
            fields.member
        );
        return;
    };
    let context = format!("get_component_ref::{}::{}", fields.class_name, fields.member);
    let actor_operand = *args.add(1);
    let Some(result) = with_world(CompOpKind::GetRef, |world| {
        // A DANGLING bit pattern must not reach a validated accessor (B3).
        let actor = match fields.target {
            RefTarget::SelfActor => context_snapshot().and_then(|(_, entity)| entity),
            RefTarget::RefPin => unsafe { read_json_value(actor_operand) }
                .and_then(|actor_json| serde_json::from_value::<u64>(actor_json).ok())
                .map(Entity::from_bits)
                .filter(|actor| *actor != Entity::DANGLING),
        };
        let Some(actor) = actor else {
            tracing::error!(
                "blueprint {context}: no actor available (unbound instance or bad operand)"
            );
            return JsonValue::Null;
        };
        crate::script_refs::component_ref_json(world, actor, &fields.class_name, component_index, &context)
    }) else {
        return;
    };
    if !ret.is_null() {
        write_json_blob(ret, &result.to_string());
    }
}

/// Shared shape of the find-object resolver trampolines (#654): one string
/// needle operand in, the found actor's packed bits out.
unsafe fn find_trampoline(
    needle: *const u8,
    ret: *mut u8,
    op: CompOpKind,
    label: &str,
    lookup: fn(&World, &JsonValue, &str) -> JsonValue,
) {
    let needle_value = unsafe { read_json_value(needle) };
    let Some(needle_value) = needle_value else {
        tracing::error!("blueprint {label}: needle operand is not valid JSON");
        return;
    };
    let Some(result) = with_world(op, |world| lookup(world, &needle_value, label)) else {
        return;
    };
    if !ret.is_null() {
        write_json_blob(ret, &result.to_string());
    }
}

/// Trampoline for `find_object_by_stable_id` (#654). Operands: `[needle]`.
pub unsafe extern "C" fn find_by_stable_id_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    find_trampoline(
        *args,
        ret,
        CompOpKind::FindByStableId,
        "find_object_by_stable_id",
        crate::script_refs::find_object_by_stable_id,
    );
}

/// Trampoline for `find_object_by_name` (#654). Operands: `[needle]`.
pub unsafe extern "C" fn find_by_name_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    find_trampoline(
        *args,
        ret,
        CompOpKind::FindByName,
        "find_object_by_name",
        crate::script_refs::find_object_by_name,
    );
}

/// Trampoline for `object_ref_literal` (#654). Operands: `[literal]`.
///
/// The staged operand is the literal's save/load form
/// (`{stable_id, class_name, component_index}`); the emitted blob carries
/// the CURRENT entity bits, resolved lazily at execution time so the
/// reference survives reloads.
pub unsafe extern "C" fn object_literal_trampoline(
    args: *const *const u8,
    ret: *mut u8,
    _type_slots: *const pbgc::bytecode::TypeSlot,
) {
    let literal = unsafe { read_json_value(*args) };
    let Some(literal) = literal else {
        tracing::error!("blueprint object_ref_literal: operand is not valid JSON");
        return;
    };
    let parsed = serde_json::from_value::<LiteralShape>(literal);
    let Ok(LiteralShape { stable_id, class_name, component_index }) = parsed else {
        tracing::error!(
            "blueprint object_ref_literal: operand does not match {{stable_id, class_name, component_index}}"
        );
        return;
    };
    let Some(result) = with_world(CompOpKind::ObjectLiteral, |world| {
        crate::script_refs::object_literal_json(
            world,
            &stable_id,
            &class_name,
            component_index,
            "object_ref_literal",
        )
    }) else {
        return;
    };
    if !ret.is_null() {
        write_json_blob(ret, &result.to_string());
    }
}

#[derive(serde::Deserialize)]
struct LiteralShape {
    stable_id: String,
    #[serde(default)]
    class_name: String,
    #[serde(default)]
    component_index: u32,
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
