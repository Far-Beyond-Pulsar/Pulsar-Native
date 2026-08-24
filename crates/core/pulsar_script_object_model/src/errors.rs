//! Typed error taxonomy for every script-facing accessor (#641).
//!
//! The contract: **accessors never panic and never silently misaddress.**
//! Every failure mode a script can hit is a variant here, `Display`-formatted
//! for editor surfaces, `Eq`/`Clone` so scripts can match on them cheaply.
//!
//! Two failure *classes* live side by side, and the distinction matters:
//!
//! - **Ordinary staleness** (`ReferenceDespawned`, `ComponentMissing`,
//!   `InstanceMissing`) is an expected runtime outcome -- handles outlive
//!   their targets by design. These are plain `Err` returns in every build.
//! - **Misuse** (passing [`pulsar_scenedb::Entity::DANGLING`] or an
//!   out-of-range slot as a live handle) returns the same
//!   `ReferenceDespawned` error; hardening with debug-build misuse
//!   assertions lands with the #641 contract work.

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
    #[error(
        "component index {component_index} on {entity} holds '{found}', not '{expected}'"
    )]
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
    UnknownProperty { class_name: String, property: String },

    /// The class has no blueprint-callable method with that name.
    #[error("'{class_name}' has no callable method '{method}'")]
    UnknownMethod { class_name: String, method: String },

    /// A value could not move between its JSON and typed representations
    /// (unregistered property type, deserialize failure, serialization
    /// failure). The underlying reflection error message is preserved.
    #[error("value marshalling failed for '{context}': {message}")]
    Marshalling { context: String, message: String },
}

/// Convenience constructor used across the crate: the common staleness case
/// only needs the handle.
impl ScriptRefError {
    pub(crate) fn despawned(entity: Entity) -> Self {
        Self::ReferenceDespawned { entity_bits: entity.bits() }
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
        assert_eq!(err.to_string(), "referenced actor Entity(3v2) no longer exists in this world");

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
}
