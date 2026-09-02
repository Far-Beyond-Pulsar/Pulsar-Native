//! Typed error taxonomy for every script-facing accessor over live
//! `World` components (#641, extended by #643).
//!
//! The contract: **accessors never panic and never silently misaddress.**
//! Every failure mode a script can hit is a variant here, `Display`-formatted
//! for editor surfaces, `Eq`/`Clone` so scripts can match on them cheaply.
//!
//! Canonical home is THIS crate (next to [`crate::dispatch`], whose failures
//! these are); `pulsar_script_object_model::errors` re-exports it so that
//! crate's public API is unchanged. Both crates' accessors and D/E boundaries
//! return this one enum -- there is no parallel taxonomy anywhere downstream.
//!
//! Two failure *classes* live side by side, and the distinction matters:
//!
//! - **Ordinary staleness** ([`ScriptRefError::ReferenceDespawned`],
//!   [`ScriptRefError::ComponentMissing`], [`ScriptRefError::InstanceMissing`])
//!   is an expected runtime outcome -- handles outlive their targets by
//!   design. These are plain `Err` returns in every build.
//! - **Misuse** (passing [`pulsar_scenedb::Entity::DANGLING`] as a live
//!   handle) returns the same `ReferenceDespawned` error in release builds
//!   and additionally trips a debug-build assertion -- a sentinel reaching
//!   an accessor can only mean raw ids crossed a boundary unconverted.

use pulsar_scenedb::Entity;

/// Everything that can go wrong through a script-facing reference.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScriptRefError {
    /// The referenced entity is dead: despawned, its slot recycled to
    /// another object, or the id never existed in this world (dangling /
    /// foreign-world ids land here too).
    #[error("referenced actor {} no longer exists in this world", pulsar_scenedb::Entity::from_bits(*.entity_bits))]
    ReferenceDespawned {
        /// The dead handle's packed bits, for diagnostics only -- never
        /// re-resolve from these; keep the original ref instead.
        entity_bits: u64,
    },

    /// The entity is alive but holds no live-typed value of the class the
    /// ref names (never hydrated, removed, disabled, or not
    /// World-registered on this entity).
    #[error("actor {entity} has no live '{class_name}' component")]
    ComponentMissing { entity: Entity, class_name: String },

    /// The instance record at `component_index` belongs to a different
    /// class than the ref claims. Same discipline as the properties panel's
    /// "index IS the identity" refusal (#519): a stale/mismatched index
    /// must never land an edit into some OTHER instance's storage.
    #[error("component index {component_index} on {entity} holds '{found}', not '{expected}'")]
    ClassMismatch {
        expected: String,
        found: String,
        component_index: u32,
        entity: Entity,
    },

    /// No component instance exists at that `(entity, component_index)`
    /// position (out of range, or no duplicate-instance storage was supplied
    /// to route a non-live index through).
    #[error("no '{class_name}' instance at index {component_index} on {entity}")]
    InstanceMissing {
        entity: Entity,
        class_name: String,
        component_index: u32,
    },

    /// The class name isn't registered for live `World` residency at all
    /// (`#[register_world_component]` never ran for it), so there is no
    /// typed value any script path could address.
    #[error("class '{0}' is not registered for live World residency")]
    UnregisteredClass(String),

    /// The class claims `World` registration but its typed-bridge shims
    /// (`get_as_engine_class*`) failed -- a registration bug, surfaced here
    /// instead of silently falling back to some other storage.
    #[error("class '{0}' has no working typed World bridge (incomplete registration)")]
    ClassNotBridged(String),

    /// The class has no reflected property with that name (checked against
    /// `PropertyMetadata`, the same metadata the properties panel renders).
    #[error("'{class_name}' has no reflected property '{property}'")]
    UnknownProperty {
        class_name: String,
        property: String,
    },

    /// The class has no blueprint-callable method with that name.
    #[error("'{class_name}' has no callable method '{method}'")]
    UnknownMethod { class_name: String, method: String },

    /// #643: the argument vector has the wrong length for the reflected
    /// method. Checked BEFORE dispatch because generated caller closures
    /// panic on missing arguments -- the dispatcher refuses first.
    #[error("'{class_name}.{method}' expects {expected} argument(s), got {got}")]
    ArgumentCount {
        class_name: String,
        method: String,
        expected: usize,
        got: usize,
    },

    /// #643: argument `index` holds a different concrete type than the
    /// reflected parameter. Checked via `TypeId` equality -- exactly the
    /// match the generated downcast would demand -- so a mismatch here is a
    /// mismatch there, reported as an `Err` instead of a panic.
    #[error(
        "argument {index} ('{param}') of '{class_name}.{method}' must be '{expected}', got '{found}'"
    )]
    ArgumentType {
        class_name: String,
        method: String,
        index: usize,
        param: &'static str,
        expected: &'static str,
        found: String,
    },

    /// A value could not move between its JSON and typed representations
    /// (unregistered property type, deserialize failure, serialization
    /// failure). The underlying reflection error message is preserved.
    #[error("value marshalling failed for '{context}': {message}")]
    Marshalling { context: String, message: String },
}

/// Convenience constructor used across both script-facing crates: the
/// common staleness case only needs the handle. (`pub` because
/// `pulsar_script_object_model` re-exports this taxonomy and builds the
/// same error from its own liveness gates.)
impl ScriptRefError {
    pub fn despawned(entity: Entity) -> Self {
        Self::ReferenceDespawned {
            entity_bits: entity.bits(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #641: error Display strings are human-readable for editor surfaces
    /// and problem panels -- no `Debug`-dump leakage.
    #[test]
    fn display_strings_name_the_target_and_cause() {
        let e = Entity::from_bits(0x0000_0002_0000_0003);
        let err = ScriptRefError::despawned(e);
        assert_eq!(
            err.to_string(),
            "referenced actor Entity(3v2) no longer exists in this world"
        );

        let err = ScriptRefError::ComponentMissing {
            entity: e,
            class_name: "LightComponent".into(),
        };
        assert_eq!(
            err.to_string(),
            "actor Entity(3v2) has no live 'LightComponent' component"
        );

        let err = ScriptRefError::ClassMismatch {
            expected: "LightComponent".into(),
            found: "StaticMeshComponent".into(),
            component_index: 2,
            entity: e,
        };
        assert_eq!(
            err.to_string(),
            "component index 2 on Entity(3v2) holds 'StaticMeshComponent', not 'LightComponent'"
        );
    }

    /// #641: errors are plain data -- cloneable and comparable so scripts
    /// can match on them without formatting gymnastics.
    #[test]
    fn errors_are_clone_and_eq() {
        let err = ScriptRefError::UnregisteredClass("NotAReal".into());
        assert_eq!(err.clone(), err);
    }

    /// #643: the argument-mismatch variants render complete diagnostics --
    /// arity names the counts, type names both sides of the mismatch.
    #[test]
    fn argument_mismatch_display_is_actionable() {
        let err = ScriptRefError::ArgumentCount {
            class_name: "TestGizmo".into(),
            method: "add_charges".into(),
            expected: 1,
            got: 0,
        };
        assert_eq!(
            err.to_string(),
            "'TestGizmo.add_charges' expects 1 argument(s), got 0"
        );

        let err = ScriptRefError::ArgumentType {
            class_name: "TestGizmo".into(),
            method: "add_charges".into(),
            index: 0,
            param: "amount",
            expected: "i32",
            found: "alloc::string::String".into(),
        };
        assert_eq!(
            err.to_string(),
            "argument 0 ('amount') of 'TestGizmo.add_charges' must be 'i32', got 'alloc::string::String'"
        );
    }
}
