//! The class `prefab.json` read model.
//!
//! The Blueprint editor writes `prefab.json`; this is the engine's view of
//! it. Every component entry has a stable **slot id**, the name overrides,
//! child objects and `get_component_ref` nodes use for it. Files written
//! before slot ids existed get deterministic ids on load
//! ([`PrefabAsset::fill_missing_slot_ids`]): `<Class>_<n>`, where `n` counts
//! earlier entries of the same class. The Blueprint editor applies the same
//! rule, so the ids it later persists match the ones levels already use.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Prefab file name inside a class directory.
pub const PREFAB_FILE: &str = "prefab.json";

/// A class prefab: its components and blueprint defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PrefabAsset {
    #[serde(default)]
    pub prefab_version: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub components: Vec<PrefabComponent>,
    #[serde(default)]
    pub blueprint_class: Option<BlueprintClassRef>,
    /// Everything else (`script_graph`, …), kept so a rewrite loses nothing.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One component of a prefab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PrefabComponent {
    /// Stable id of this slot within the class. Empty in files written
    /// before slot ids existed; see [`PrefabAsset::fill_missing_slot_ids`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub slot_id: String,
    pub class_name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub data: Value,
}

fn default_true() -> bool {
    true
}

/// Blueprint attachment of a prefab and its variable defaults.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BlueprintClassRef {
    #[serde(default)]
    pub class_path: String,
    #[serde(default)]
    pub variable_defaults: HashMap<String, Value>,
}

impl PrefabAsset {
    /// Read `<dir>/prefab.json`. A class without one has no components.
    pub fn load_from_dir(dir: &Path) -> Result<Self, String> {
        let path = dir.join(PREFAB_FILE);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mut prefab: Self = serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        prefab.fill_missing_slot_ids();
        Ok(prefab)
    }

    /// Give every component without a slot id (or with a duplicate one) a
    /// deterministic id. Returns whether anything changed.
    pub fn fill_missing_slot_ids(&mut self) -> bool {
        let mut used: HashSet<String> = HashSet::new();
        let mut needs: Vec<usize> = Vec::new();
        for (index, component) in self.components.iter().enumerate() {
            if component.slot_id.trim().is_empty() || !used.insert(component.slot_id.clone()) {
                needs.push(index);
            }
        }
        if needs.is_empty() {
            return false;
        }
        for index in needs {
            let class = self.components[index].class_name.clone();
            let occurrence = self.components[..index]
                .iter()
                .filter(|c| c.class_name == class)
                .count();
            let id = next_free_slot_id(&class, occurrence, &used);
            used.insert(id.clone());
            self.components[index].slot_id = id;
        }
        true
    }

    /// The component in slot `slot_id`.
    pub fn slot(&self, slot_id: &str) -> Option<&PrefabComponent> {
        self.components.iter().find(|c| c.slot_id == slot_id)
    }

    /// Variable defaults declared by the blueprint class.
    pub fn variable_defaults(&self) -> HashMap<String, Value> {
        self.blueprint_class
            .as_ref()
            .map(|b| b.variable_defaults.clone())
            .unwrap_or_default()
    }
}

/// `<class>_<n>` for the smallest `n >= start` not in `used`.
pub fn next_free_slot_id(class_name: &str, start: usize, used: &HashSet<String>) -> String {
    let mut n = start;
    loop {
        let candidate = format!("{class_name}_{n}");
        if !used.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_prefabs_get_deterministic_slot_ids() {
        let json = r#"{
            "prefab_version": 1, "name": "Lamp",
            "components": [
                { "class_name": "LightComponent", "enabled": true, "data": {} },
                { "class_name": "StaticMeshComponent", "enabled": true, "data": {} },
                { "class_name": "LightComponent", "enabled": true, "data": {} }
            ],
            "script_graph": { "nodes": [] }
        }"#;
        let mut a: PrefabAsset = serde_json::from_str(json).unwrap();
        let mut b: PrefabAsset = serde_json::from_str(json).unwrap();
        assert!(a.fill_missing_slot_ids());
        b.fill_missing_slot_ids();
        let ids: Vec<_> = a.components.iter().map(|c| c.slot_id.clone()).collect();
        assert_eq!(
            ids,
            [
                "LightComponent_0",
                "StaticMeshComponent_0",
                "LightComponent_1"
            ]
        );
        assert_eq!(
            ids,
            b.components
                .iter()
                .map(|c| c.slot_id.clone())
                .collect::<Vec<_>>(),
            "same file, same ids"
        );
        assert!(!a.fill_missing_slot_ids(), "idempotent");
        // Unknown sections survive a round trip.
        let text = serde_json::to_string(&a).unwrap();
        assert!(text.contains("script_graph"));
        assert!(text.contains("\"slot_id\":\"LightComponent_1\""));
    }

    #[test]
    fn existing_ids_are_kept_and_duplicates_repaired() {
        let mut prefab = PrefabAsset {
            components: vec![
                PrefabComponent {
                    slot_id: "LightComponent_0".into(),
                    class_name: "LightComponent".into(),
                    enabled: true,
                    data: Value::Null,
                },
                PrefabComponent {
                    slot_id: "LightComponent_0".into(),
                    class_name: "LightComponent".into(),
                    enabled: true,
                    data: Value::Null,
                },
                PrefabComponent {
                    slot_id: String::new(),
                    class_name: "LightComponent".into(),
                    enabled: true,
                    data: Value::Null,
                },
            ],
            ..Default::default()
        };
        prefab.fill_missing_slot_ids();
        let ids: Vec<_> = prefab
            .components
            .iter()
            .map(|c| c.slot_id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["LightComponent_0", "LightComponent_1", "LightComponent_2"]
        );
    }
}
