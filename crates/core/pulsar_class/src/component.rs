//! The `ClassInstance` world component.

use std::collections::BTreeMap;

use engine_class_derive::{engine_class, register_runtime_behavior, register_world_component};
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};
use serde_json::Value;

use crate::id::ClassId;

/// Marks an entity as a placed instance of a class.
///
/// The instance *references* its class: the class's prefab components are
/// created from the current class definition whenever the instance is
/// built (placement, level load), and only the values that differ from the
/// class defaults are stored here.
///
/// - `variable_overrides`: script variable name → JSON value.
/// - `component_overrides`: prefab slot id → JSON diff against the slot's
///   default data (only the changed properties, nested objects diffed
///   recursively). `{"__removed": true}` marks a slot the instance removed.
///
/// `class_name` is a hint: it names the class when the GUID cannot be
/// resolved (a missing class, or a level migrated before the class had an
/// id), so an unresolved instance keeps its data and can still be matched.
#[engine_class(category = "Scripting", default, clone, debug, serialize, deserialize)]
pub struct ClassInstance {
    #[serde(default)]
    pub class: ClassId,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub class_name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variable_overrides: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub component_overrides: BTreeMap<String, Value>,
}

impl ClassInstance {
    /// An instance of `class` with no overrides.
    pub fn new(class: ClassId, class_name: impl Into<String>) -> Self {
        Self {
            class,
            class_name: class_name.into(),
            variable_overrides: BTreeMap::new(),
            component_overrides: BTreeMap::new(),
        }
    }

    /// Parse from a component's JSON data.
    pub fn from_json(data: &Value) -> Option<Self> {
        serde_json::from_value(data.clone()).ok()
    }

    /// This instance as component JSON data.
    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

// The component itself has no runtime behaviour yet: the script runtime
// follows it in phase B (#922). Registering it makes it a typed World
// component that saves, loads and reaches scripts like every other one.
#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for ClassInstance {
    const CLASS_NAME: &'static str = "ClassInstance";

    fn sync_component(
        _owner: &RuntimeComponentOwner,
        _component_index: usize,
        _component: &Self,
        _context: &mut dyn ComponentRuntimeContext,
    ) {
    }
}
