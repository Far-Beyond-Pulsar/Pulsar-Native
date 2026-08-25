//! Access-routing internals: which storage a [`ComponentRef`] addresses.
//!
//! The single decision every accessor funnels through: given the handle's
//! `(class_name, component_index)` identity and an optional
//! [`ComponentInstanceStore`], does this access go to the **live-typed**
//! `World` value (the panel's first-enabled instance) or to a specific
//! **duplicate instance's own JSON record**? The index is the identity --
//! it selects the storage before any presence check, so a stale or
//! mismatched index can never land an edit into some OTHER instance's
//! storage (Pulsar-Native#519).
//!
//! Also home to the throwaway-world hydration mechanism duplicates are read
//! and edited through -- the panel's exact approach (Pulsar-Native#561):
//! real typed machinery against the instance's own JSON, never fragile
//! partial merges.

use pulsar_scenedb::World;
use pulsar_world_registry::{
    component_id_for_class, get_world_component_as_engine_class,
    get_world_component_as_engine_class_mut, hydrate_world_component_for_class,
};
use serde_json::Value;

use crate::errors::ScriptRefError;
use crate::instances::{ComponentInstanceStore, InstanceRecord};
use crate::refs::{ensure_live_entity, ComponentRef};

/// Which storage an access routes through.
pub(crate) enum Route {
    /// The live-typed value resident in the World itself.
    Live,
    /// One duplicate/disabled instance's own serialized record.
    Duplicate { record: InstanceRecord },
}

/// Decide which storage `r` addresses RIGHT NOW. See module doc for the
/// index-is-identity discipline; errors are typed per [`crate::errors`].
pub(crate) fn route(
    r: &ComponentRef,
    world: &World,
    store: Option<&dyn ComponentInstanceStore>,
) -> Result<Route, ScriptRefError> {
    ensure_live_entity(world, r.entity)?;
    if component_id_for_class(&r.class_name).is_none() {
        return Err(ScriptRefError::UnregisteredClass(r.class_name.clone()));
    }

    // First-enabled index per the store's list; without a store the runtime
    // convention applies: the single typed row IS index 0 (hydration
    // collapses duplicates into one live value).
    let live_index =
        store.and_then(|s| s.live_component_index(r.entity, &r.class_name)).unwrap_or(0);
    if r.component_index == live_index {
        return if get_world_component_as_engine_class(&r.class_name, world, r.entity).is_some() {
            Ok(Route::Live)
        } else {
            Err(ScriptRefError::ComponentMissing { entity: r.entity, class_name: r.class_name.clone() })
        };
    }

    let Some(record) = store.and_then(|s| s.instance_record(r.entity, r.component_index)) else {
        return Err(ScriptRefError::InstanceMissing {
            entity: r.entity,
            class_name: r.class_name.clone(),
            component_index: r.component_index,
        });
    };
    if record.class_name != r.class_name {
        return Err(ScriptRefError::ClassMismatch {
            expected: r.class_name.clone(),
            found: record.class_name,
            component_index: r.component_index,
            entity: r.entity,
        });
    }
    Ok(Route::Duplicate { record })
}

/// A hydrated throwaway world holding one component instance -- the panel's
/// exact mechanism for editing duplicate/disabled instances with the real
/// typed machinery instead of fragile JSON merges (Pulsar-Native#561).
pub(crate) struct ScratchInstance {
    world: World,
    entity: pulsar_scenedb::Entity,
    class_name: String,
}

impl ScratchInstance {
    /// Hydrate `class_name` from one instance's full JSON into the scratch
    /// world. `Err(Marshalling)` when the JSON doesn't deserialize for its
    /// own class -- refused, never guessed at.
    pub fn hydrate(class_name: &str, data: &Value) -> Result<Self, ScriptRefError> {
        let mut world = World::new();
        let entity = world.spawn();
        hydrate_world_component_for_class(class_name, &mut world, entity, data).map_err(
            |message| ScriptRefError::Marshalling {
                context: format!("{class_name} instance JSON"),
                message,
            },
        )?;
        Ok(Self { world, entity, class_name: class_name.to_string() })
    }

    pub fn instance(&self) -> Result<&dyn pulsar_reflection::EngineClass, ScriptRefError> {
        get_world_component_as_engine_class(&self.class_name, &self.world, self.entity)
            .ok_or_else(|| ScriptRefError::ClassNotBridged(self.class_name.clone()))
    }

    pub fn instance_mut(
        &mut self,
    ) -> Result<&mut dyn pulsar_reflection::EngineClass, ScriptRefError> {
        get_world_component_as_engine_class_mut(&self.class_name, &mut self.world, self.entity)
            .ok_or_else(|| ScriptRefError::ClassNotBridged(self.class_name.clone()))
    }

    /// Serialize the (possibly edited) instance back to whole-component
    /// JSON. `Err` (never a silent `Null` overwrite) when the JSON round
    /// trip is unavailable.
    pub fn persist(&self) -> Result<Value, ScriptRefError> {
        let json = self.instance()?.to_json().map_err(|message| ScriptRefError::Marshalling {
            context: format!("{} persist", self.class_name),
            message,
        })?;
        Ok(json)
    }
}

/// Serialize one property's typed value to JSON via the runtime registry.
pub(crate) fn serialize_property(
    class_name: &str,
    property: &str,
    value: &dyn std::any::Any,
) -> Result<Value, ScriptRefError> {
    pulsar_reflection::RUNTIME_TYPE_REGISTRY
        .serialize_json_for_any(value)
        .map_err(|e| ScriptRefError::Marshalling {
            context: format!("{class_name}.{property}"),
            message: e.to_string(),
        })
}

/// Deserialize one property's JSON against its reflected type via the
/// runtime registry.
pub(crate) fn deserialize_property(
    class_name: &str,
    property: &str,
    type_info: &pulsar_reflection::RuntimeTypeInfo,
    value: Value,
) -> Result<Box<dyn std::any::Any>, ScriptRefError> {
    pulsar_reflection::RUNTIME_TYPE_REGISTRY
        .deserialize_json_for_type(type_info, value)
        .map_err(|e| ScriptRefError::Marshalling {
            context: format!("{class_name}.{property}"),
            message: e.to_string(),
        })
}
