//! The unified reflection dispatcher (#643): invoke any registered reflected
//! method -- or read/write any reflected property -- against the live-typed
//! component value in a [`World`], knowing nothing but
//! `(world, entity, class_name, component_index, names, values)`.
//!
//! This is the keystone every scripting backend calls instead of bespoke
//! dispatch: D's `comp_*` opcodes, E's generated code, and F's graph nodes
//! all funnel through [`invoke_component_method`] /
//! [`get_component_property`] / [`set_component_property`]. It composes the
//! proven pieces only -- `pulsar_reflection::REGISTRY.get_method`'s
//! `MethodMetadata.caller` closures and this crate's
//! `get_world_component_as_engine_class_mut` bridge -- inventing no new
//! resolution path, and reuses the one script-facing error taxonomy
//! ([`ScriptRefError`], shared with `pulsar_script_object_model`).
//!
//! ## Invariants
//!
//! - **Never panics on bad input.** Generated caller closures themselves
//!   panic on argument-count/type mismatches, so the dispatcher validates
//!   arity and exact `TypeId` match BEFORE handing args to a closure and
//!   reports [`ScriptRefError::ArgumentCount`]/[`ArgumentType`] instead.
//! - **Panel-parity identity** (#519). `(class_name, component_index)`:
//!   index 0 is the live-typed value. Property access through THIS layer
//!   rejects other indexes with [`ScriptRefError::InstanceMissing`]
//!   (duplicate records live behind the object-model crate's
//!   `ComponentInstanceStore`, not here). Methods are class-level behavior
//!   shared by every duplicate instance, so they execute against the
//!   live-typed value regardless of index -- the semantics
//!   `ComponentRef::call_method` established.
//! - **Mutations ride the real storage.** Setters go through the typed
//!   World bridge, so SceneDB's `Mut` guards fire subscription/GPU events
//!   exactly like properties-panel edits.

use std::any::Any;

use pulsar_reflection::{
    MethodArgs, MethodMetadata, MethodReturnValue, PropertyMetadata, RuntimeTypeInfo, REGISTRY,
    RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use crate::errors::ScriptRefError;

/// Invoke one blueprint-callable method on an entity's live-typed component
/// value.
///
/// Resolution order (each step a typed error, never a panic):
/// 1. entity liveness ([`ScriptRefError::ReferenceDespawned`]),
/// 2. live World registration for `class_name`
///    ([`ScriptRefError::UnregisteredClass`]),
/// 3. method metadata lookup ([`ScriptRefError::UnknownMethod`]),
/// 4. argument arity + exact-type validation
///    ([`ScriptRefError::ArgumentCount`]/[`ScriptRefError::ArgumentType`]) --
///    performed HERE because the generated callers panic otherwise,
/// 5. presence of the live-typed value ([`ScriptRefError::ComponentMissing`]),
/// 6. `caller(args)` against `&mut dyn EngineClass`.
///
/// `Ok(None)` is a valid result: the method ran and returned nothing.
pub fn invoke_component_method(
    world: &mut World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
    method: &str,
    args: MethodArgs,
) -> Result<MethodReturnValue, ScriptRefError> {
    ensure_live_entity(world, entity)?;
    if crate::component_id_for_class(class_name).is_none() {
        return Err(ScriptRefError::UnregisteredClass(class_name.to_string()));
    }
    let meta = REGISTRY
        .get_method(class_name, method)
        .ok_or_else(|| ScriptRefError::UnknownMethod {
            class_name: class_name.to_string(),
            method: method.to_string(),
        })?;
    validate_args(class_name, &meta, &args)?;

    let _ = component_index; // methods are class-level behavior; see module doc
    let instance = crate::get_world_component_as_engine_class_mut(class_name, world, entity)
        .ok_or_else(|| ScriptRefError::ComponentMissing {
            entity,
            class_name: class_name.to_string(),
        })?;
    Ok((meta.caller)(&mut *instance, args))
}

/// Read one reflected property of an entity's live-typed component value as
/// JSON (the editor/metadata representation).
///
/// Only the live-typed instance (index 0 at this layer -- duplicate records
/// route through the object-model crate) is addressable here.
pub fn get_component_property(
    world: &World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
    property: &str,
) -> Result<Value, ScriptRefError> {
    let meta = property_metadata(class_name, property)?;
    let instance = live_instance(world, entity, class_name, component_index)?;
    let value = (meta.getter)(instance);
    crate::marshal::any_to_json(&property_context(class_name, property), &*value)
}

/// Read one reflected property as a typed `Box<dyn Any>` -- the no-JSON hot
/// path (#644/#D4); what the VM's comp_* opcodes should prefer.
pub fn get_component_property_boxed(
    world: &World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
    property: &str,
) -> Result<Box<dyn Any>, ScriptRefError> {
    let meta = property_metadata(class_name, property)?;
    let instance = live_instance(world, entity, class_name, component_index)?;
    Ok((meta.getter)(instance))
}

/// Write one reflected property of an entity's live-typed component value
/// from JSON. Nothing is written on failure.
///
/// The value deserializes against the property's reflected type FIRST, so a
/// malformed value is a typed [`ScriptRefError::Marshalling`] error and the
/// component is untouched.
pub fn set_component_property(
    world: &mut World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
    property: &str,
    value: Value,
) -> Result<(), ScriptRefError> {
    let meta = property_metadata(class_name, property)?;
    let typed =
        crate::marshal::json_to_any(&property_context(class_name, property), meta.type_info, value)?;
    set_typed(world, entity, class_name, component_index, meta, typed)
}

/// Write one reflected property from an already-typed `Box<dyn Any>` -- the
/// no-JSON hot path (#644/#D4). The value's concrete type must equal the
/// property's reflected type exactly (the same match the setter's own
/// downcast demands); anything else is refused, never silently ignored.
pub fn set_component_property_boxed(
    world: &mut World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
    property: &str,
    value: Box<dyn Any>,
) -> Result<(), ScriptRefError> {
    let meta = property_metadata(class_name, property)?;
    validate_arg_type(class_name, property, 0, meta.name, meta.type_info, value.as_ref())?;
    set_typed(world, entity, class_name, component_index, meta, value)
}

/// Convert graph-domain JSON argument values into a method's declared
/// parameter types, driven by reflection metadata.
///
/// The shared front half of [`invoke_component_method`] for callers whose
/// values start as JSON — the VM's comp_call trampoline and PBGC's generated
/// actors both stage arguments this way, so both cross into typed dispatch
/// identically. Arity is validated here ([`ScriptRefError::ArgumentCount`]);
/// each value deserializes against its parameter's reflected type
/// ([`ScriptRefError::Marshalling`] on mismatch, nothing partially
/// converted). Unknown methods are [`ScriptRefError::UnknownMethod`], never a
/// silent empty arg list.
pub fn json_args_to_method_args(
    class_name: &str,
    method: &str,
    values: Vec<Value>,
) -> Result<MethodArgs, ScriptRefError> {
    let meta = REGISTRY.get_method(class_name, method).ok_or_else(|| {
        ScriptRefError::UnknownMethod {
            class_name: class_name.to_string(),
            method: method.to_string(),
        }
    })?;
    if values.len() != meta.params.len() {
        return Err(ScriptRefError::ArgumentCount {
            class_name: class_name.to_string(),
            method: meta.name.to_string(),
            expected: meta.params.len(),
            got: values.len(),
        });
    }
    meta.params
        .iter()
        .zip(values)
        .map(|(param, value)| {
            crate::marshal::json_to_any(
                &format!("{class_name}.{} (arg {})", meta.name, param.name),
                param.type_info,
                value,
            )
        })
        .collect()
}

// ── shared internals ───────────────────────────────────────────────────────

fn property_context(class_name: &str, property: &str) -> String {
    format!("{class_name}.{property}")
}

fn set_typed(
    world: &mut World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
    meta: PropertyMetadata,
    typed: Box<dyn Any>,
) -> Result<(), ScriptRefError> {
    let instance = live_instance_mut(world, entity, class_name, component_index)?;
    (meta.setter)(&mut *instance, typed);
    Ok(())
}

/// Reflected metadata for one property, resolved through a throwaway default
/// instance exactly like the properties panel does -- only the type-bound
/// getter/setter closures are used, never the throwaway's values.
fn property_metadata(
    class_name: &str,
    property: &str,
) -> Result<PropertyMetadata, ScriptRefError> {
    REGISTRY
        .create_instance(class_name)
        .and_then(|instance| {
            instance.get_properties().into_iter().find(|p| p.name == property)
        })
        .ok_or_else(|| ScriptRefError::UnknownProperty {
            class_name: class_name.to_string(),
            property: property.to_string(),
        })
}

fn live_instance<'w>(
    world: &'w World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
) -> Result<&'w dyn pulsar_reflection::EngineClass, ScriptRefError> {
    if component_index != 0 {
        return Err(ScriptRefError::InstanceMissing {
            entity,
            class_name: class_name.to_string(),
            component_index,
        });
    }
    crate::get_world_component_as_engine_class(class_name, world, entity)
        .ok_or_else(|| ScriptRefError::ComponentMissing {
            entity,
            class_name: class_name.to_string(),
        })
}

fn live_instance_mut<'w>(
    world: &'w mut World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
) -> Result<&'w mut dyn pulsar_reflection::EngineClass, ScriptRefError> {
    if component_index != 0 {
        return Err(ScriptRefError::InstanceMissing {
            entity,
            class_name: class_name.to_string(),
            component_index,
        });
    }
    crate::get_world_component_as_engine_class_mut(class_name, world, entity)
        .ok_or_else(|| ScriptRefError::ComponentMissing {
            entity,
            class_name: class_name.to_string(),
        })
}

/// Liveness gate mirroring the object-model crate's `ensure_live_entity`:
/// ordinary staleness is a plain typed error in every build;
/// `Entity::DANGLING` additionally trips a debug assert (raw-id abuse).
fn ensure_live_entity(world: &World, entity: Entity) -> Result<(), ScriptRefError> {
    if entity == Entity::DANGLING {
        debug_assert!(
            false,
            "script dispatcher misuse: Entity::DANGLING reached a liveness-checked accessor \
             (raw-id abuse across a language boundary, not ordinary staleness)"
        );
        return Err(ScriptRefError::despawned(entity));
    }
    if !world.is_alive(entity) {
        return Err(ScriptRefError::despawned(entity));
    }
    Ok(())
}

/// Arity + exact-type validation, performed before any caller runs (the
/// generated closures panic on both conditions). Type checking compares
/// `TypeId`s -- byte-for-byte the match the generated `downcast::<T>()`
/// demands -- so validation can never pass where dispatch would panic.
fn validate_args(
    class_name: &str,
    meta: &MethodMetadata,
    args: &MethodArgs,
) -> Result<(), ScriptRefError> {
    if args.len() != meta.params.len() {
        return Err(ScriptRefError::ArgumentCount {
            class_name: class_name.to_string(),
            method: meta.name.to_string(),
            expected: meta.params.len(),
            got: args.len(),
        });
    }
    // NOTE: `arg.as_ref()` is load-bearing -- `Box<dyn Any>` is itself
    // `Any`, so `.type_id()` on the box would report the box, not the
    // payload (and every downcast would look like a mismatch).
    for (index, (arg, param)) in args.iter().zip(&meta.params).enumerate() {
        validate_arg_type(class_name, meta.name, index, param.name, param.type_info, arg.as_ref())?;
    }
    Ok(())
}

fn validate_arg_type(
    class_name: &str,
    method: &str,
    index: usize,
    param: &'static str,
    expected: &'static RuntimeTypeInfo,
    arg: &dyn Any,
) -> Result<(), ScriptRefError> {
    if arg.type_id() != expected.type_id {
        return Err(ScriptRefError::ArgumentType {
            class_name: class_name.to_string(),
            method: method.to_string(),
            index,
            param,
            expected: expected.type_name,
            found: RUNTIME_TYPE_REGISTRY
                .get_by_id(arg.type_id())
                .map(|info| info.type_name.to_string())
                .unwrap_or_else(|| format!("{:?}", arg.type_id())),
        });
    }
    Ok(())
}
