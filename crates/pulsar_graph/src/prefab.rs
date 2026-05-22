use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Serialized prefab asset stored in .prefab files.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrefabAsset {
    pub prefab_version: u32,
    pub name: String,
    pub components: Vec<ComponentInstance>,
    pub blueprint_class: Option<BlueprintClassRef>,
}

/// Single serialized component instance in a prefab.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentInstance {
    pub component_type: String,
    pub properties: HashMap<String, serde_json::Value>,
}

/// Optional blueprint attachment and default overrides.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlueprintClassRef {
    /// Path to blueprint class folder or compiled bytecode.
    pub class_path: String,
    /// Variable default overrides supplied by the prefab.
    #[serde(default)]
    pub variable_defaults: HashMap<String, serde_json::Value>,
}

impl PrefabAsset {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            prefab_version: 1,
            name: name.into(),
            components: Vec::new(),
            blueprint_class: None,
        }
    }
}
