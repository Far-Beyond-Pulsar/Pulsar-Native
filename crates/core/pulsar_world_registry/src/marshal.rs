//! Unified value marshalling (#644): ONE conversion layer between the three
//! argument representations scripting backends deal in --
//! [`serde_json::Value`] (editor/metadata path), `Box<dyn Any>` (reflection
//! caller closures), and raw arena bytes (the VM `DispatchFn` ABI).
//!
//! Every conversion is driven by the value's registered
//! [`RuntimeTypeInfo`] -- never by per-call-site ad-hoc matches -- so a type
//! registered with reflection marshals through every leg automatically.
//! All failures are reported as [`ScriptRefError::Marshalling`] with a
//! context string naming the class.property (or method) involved; nothing
//! here panics or truncates.
//!
//! Legs landed so far: JSON ⇄ `Box<dyn Any>` (this file's two functions,
//! consumed by the #643 property accessors). The bytes leg and the versioned
//! VM TypeSlot encoding live in this module family as well (#644).

use std::any::Any;

use pulsar_reflection::{RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY};
use serde_json::Value;

use crate::errors::ScriptRefError;

/// Serialize any registered reflected value to JSON via the runtime type
/// registry. `context` names the call site (e.g. `"LightComponent.color"`)
/// for the error message.
pub fn any_to_json(context: &str, value: &dyn Any) -> Result<Value, ScriptRefError> {
    RUNTIME_TYPE_REGISTRY.serialize_json_for_any(value).map_err(|e| {
        ScriptRefError::Marshalling { context: context.to_string(), message: e.to_string() }
    })
}

/// Deserialize JSON into a typed value against `type_info`'s registration.
///
/// Exactness invariant: the returned box holds EXACTLY the registered
/// concrete type (`type_info.type_id`) or this is an `Err` -- callers can
/// hand the result straight to setter closures/downcasts without a second
/// check.
pub fn json_to_any(
    context: &str,
    type_info: &'static RuntimeTypeInfo,
    value: Value,
) -> Result<Box<dyn Any>, ScriptRefError> {
    RUNTIME_TYPE_REGISTRY
        .deserialize_json_for_type(type_info, value)
        .map_err(|e| {
            ScriptRefError::Marshalling { context: context.to_string(), message: e.to_string() }
        })
}
