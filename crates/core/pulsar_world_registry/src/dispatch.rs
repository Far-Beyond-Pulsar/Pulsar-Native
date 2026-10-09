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
//! - **Instance identity** (Pulsar-Native#1035, D1). Every attached
//!   instance is its own entity holding its own typed value, so an address
//!   resolves to exactly one instance: `entity` is either that instance
//!   entity, or its owner object with `component_index` as the class-local
//!   ordinal (0 = the object's first instance of the class, enabled or
//!   not). Properties and methods act on the addressed instance; there is
//!   no "live-typed index" and no JSON copy of the others.
//! - **Mutations ride the real storage.** Setters go through the typed
//!   World bridge, so SceneDB's `Mut` guards fire subscription/GPU events
//!   exactly like properties-panel edits.

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

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
    let meta =
        REGISTRY
            .get_method(class_name, method)
            .ok_or_else(|| ScriptRefError::UnknownMethod {
                class_name: class_name.to_string(),
                method: method.to_string(),
            })?;
    validate_args(class_name, &meta, &args)?;

    let property_written =
        crate::find(class_name).map(|registration| registration.property_written);
    let mut instance = live_instance_mut(world, entity, class_name, component_index)?;
    let result = (meta.caller)(&mut *instance, args);
    // The method may have written any field.
    if let Some(property_written) = property_written {
        property_written(&mut *instance, None);
    }
    Ok(result)
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
    let meta = property_descriptor(class_name, property)?;
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
    let meta = property_descriptor(class_name, property)?;
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
    let meta = property_descriptor(class_name, property)?;
    let typed = crate::marshal::json_to_any(
        &property_context(class_name, property),
        meta.type_info,
        value,
    )?;
    set_typed(world, entity, class_name, component_index, &meta, typed)
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
    let meta = property_descriptor(class_name, property)?;
    validate_arg_type(
        class_name,
        property,
        0,
        meta.name,
        meta.type_info,
        value.as_ref(),
    )?;
    set_typed(world, entity, class_name, component_index, &meta, value)
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
    meta: &PropertyMetadata,
    typed: Box<dyn Any>,
) -> Result<(), ScriptRefError> {
    let property_written =
        crate::find(class_name).map(|registration| registration.property_written);
    let mut instance = live_instance_mut(world, entity, class_name, component_index)?;
    (meta.setter)(&mut *instance, typed);
    if let Some(property_written) = property_written {
        property_written(&mut *instance, Some(meta.name));
    }
    Ok(())
}

/// Reflected metadata for one property: its type-bound getter/setter
/// closures and type information.
///
/// Descriptors are immutable and per class, and the registry is fixed at
/// link time, so each class's are built once (through one throwaway default
/// instance, exactly as the properties panel does) and shared afterwards:
/// an access is a read lock and two hash lookups, with no `EngineClass`
/// construction. Only the type-bound closures are used, never the
/// throwaway's values. Nothing here refers to an entity or a borrowed
/// component, so the cache can never hold a stale pointer.
pub fn property_descriptor(
    class_name: &str,
    property: &str,
) -> Result<Arc<PropertyMetadata>, ScriptRefError> {
    static CACHE: LazyLock<RwLock<HashMap<String, Arc<ClassProperties>>>> =
        LazyLock::new(Default::default);

    let unknown = || ScriptRefError::UnknownProperty {
        class_name: class_name.to_string(),
        property: property.to_string(),
    };
    let cached = CACHE
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(class_name)
        .cloned();
    let class = match cached {
        Some(class) => class,
        None => {
            let instance = REGISTRY.create_instance(class_name).ok_or_else(unknown)?;
            let built: Arc<ClassProperties> = Arc::new(
                instance
                    .get_properties()
                    .into_iter()
                    .map(|p| (p.name, Arc::new(p)))
                    .collect(),
            );
            let mut cache = CACHE.write().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(cache.entry(class_name.to_owned()).or_insert(built))
        }
    };
    class.get(property).cloned().ok_or_else(unknown)
}

type ClassProperties = HashMap<&'static str, Arc<PropertyMetadata>>;

/// The instance entity an address names; see the module doc.
fn addressed_instance(
    world: &World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
) -> Result<Entity, ScriptRefError> {
    crate::instances::resolve_instance(world, entity, class_name, component_index).ok_or_else(
        || {
            if component_index == 0 {
                ScriptRefError::ComponentMissing {
                    entity,
                    class_name: class_name.to_string(),
                }
            } else {
                ScriptRefError::InstanceMissing {
                    entity,
                    class_name: class_name.to_string(),
                    component_index,
                }
            }
        },
    )
}

fn live_instance<'w>(
    world: &'w World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
) -> Result<&'w dyn pulsar_reflection::EngineClass, ScriptRefError> {
    let instance = addressed_instance(world, entity, class_name, component_index)?;
    crate::get_world_component_as_engine_class(class_name, world, instance).ok_or_else(|| {
        ScriptRefError::ComponentMissing {
            entity,
            class_name: class_name.to_string(),
        }
    })
}

fn live_instance_mut<'w>(
    world: &'w mut World,
    entity: Entity,
    class_name: &str,
    component_index: u32,
) -> Result<crate::EngineClassMut<'w>, ScriptRefError> {
    let instance = addressed_instance(world, entity, class_name, component_index)?;
    crate::get_world_component_as_engine_class_mut(class_name, world, instance).ok_or_else(|| {
        ScriptRefError::ComponentMissing {
            entity,
            class_name: class_name.to_string(),
        }
    })
}

/// Liveness gate mirroring the object-model crate's `ensure_live_entity`:
/// a plain typed error in every build. `Entity::DANGLING` is scripts'
/// `entity::none()`, so it is ordinary "not live" too (#888).
fn ensure_live_entity(world: &World, entity: Entity) -> Result<(), ScriptRefError> {
    if entity == Entity::DANGLING || !world.is_alive(entity) {
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
        validate_arg_type(
            class_name,
            meta.name,
            index,
            param.name,
            param.type_info,
            arg.as_ref(),
        )?;
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
