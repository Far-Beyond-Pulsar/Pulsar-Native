//! The `NativeScriptComponent`: binds a scene object to an actor type from a
//! Rust gameplay script crate (#653).
//!
//! Blueprint classes are placed as `ClassInstance`s; native script crates
//! are not class assets (no class directory, prefab or GUID; the crate
//! registers actor types in code), so they keep their own small component.
//! It replaces the "rust mode" of the retired `ScriptComponent`
//! (`{ "mode": "rust", "script_crate", "actor_type" }`), which the level
//! migration converts.

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};

/// Component class name of [`NativeScriptComponent`].
pub const NATIVE_SCRIPT_COMPONENT: &str = "NativeScriptComponent";

/// Runs the actor type `actor_type` from script crate `script_crate` on
/// this object.
#[engine_class(category = "Scripting", default, clone, debug, serialize, deserialize)]
pub struct NativeScriptComponent {
    /// Cargo package name of the script crate under `<project>/scripts/`.
    #[property]
    pub script_crate: String,
    /// Actor type the crate registers (`register_actor::<Type>`), short name.
    #[property]
    pub actor_type: String,
}

impl NativeScriptComponent {
    pub fn new(script_crate: impl Into<String>, actor_type: impl Into<String>) -> Self {
        Self {
            script_crate: script_crate.into(),
            actor_type: actor_type.into(),
        }
    }
}

// Spawning native actors from this binding is the script-runtime phase's
// job (#922); the component is data only for now.
#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for NativeScriptComponent {
    const CLASS_NAME: &'static str = NATIVE_SCRIPT_COMPONENT;

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
    }
}
