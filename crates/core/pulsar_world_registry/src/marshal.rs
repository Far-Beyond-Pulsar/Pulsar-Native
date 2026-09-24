//! Value marshalling between [`serde_json::Value`] (the editor/metadata
//! representation) and `Box<dyn Any>` (what reflection caller closures
//! take), driven by each value's registered [`RuntimeTypeInfo`].
//!
//! All failures are [`ScriptRefError::Marshalling`] with a context naming
//! the call site and type; nothing here panics.

use std::any::Any;

use pulsar_reflection::{RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY};
use serde_json::Value;

use crate::errors::ScriptRefError;

// ── JSON ⇄ Any ─────────────────────────────────────────────────────────────

/// Serialize any registered reflected value to JSON via the runtime type
/// registry. `context` names the call site (e.g. `"LightComponent.color"`)
/// for the error message.
pub fn any_to_json(context: &str, value: &dyn Any) -> Result<Value, ScriptRefError> {
    RUNTIME_TYPE_REGISTRY
        .serialize_json_for_any(value)
        .map_err(|e| ScriptRefError::Marshalling {
            context: context.to_string(),
            message: e.to_string(),
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
        .map_err(|e| ScriptRefError::Marshalling {
            context: context.to_string(),
            message: e.to_string(),
        })
}
