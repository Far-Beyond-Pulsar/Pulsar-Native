//! Re-export of the script-facing error taxonomy (#641/#643).
//!
//! The canonical definition moved next to the unified dispatcher it
//! describes -- `pulsar_world_registry::errors` (#643 landed
//! `invoke_component_method` there, and both crates' accessors return ONE
//! enum; there is no parallel taxonomy downstream). Every path this crate
//! published before is unchanged: `pulsar_script_object_model::ScriptRefError`
//! and `pulsar_script_object_model::errors::ScriptRefError` still resolve.

pub use pulsar_world_registry::ScriptRefError;
