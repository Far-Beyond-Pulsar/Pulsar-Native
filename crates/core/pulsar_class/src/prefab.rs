//! The class `prefab.json` read model.
//!
//! The Blueprint editor writes `prefab.json`; this is the engine's view of
//! it. Every component entry has a **slot id**: a UUID, unique within its
//! class, that the class's compiled script uses to name the component and
//! that levels key per-instance overrides by. Slot ids exist only on disk:
//! placing a class resolves each one once into a handle to the instance's
//! real component (see `crate::world::ClassPlacement`).
//!
//! A prefab without slot ids, or with ids that are not UUIDs (the readable
//! `<Class>_<n>` ids of early #921 builds), gets fresh UUIDs on load, and
//! the file is rewritten once so the ids stay stable
//! ([`PrefabAsset::load_from_dir`]).

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
    /// UUID of this slot, unique within the class. Empty in files written
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

/// Whether `slot_id` is a valid slot id (a UUID).
pub fn is_slot_uuid(slot_id: &str) -> bool {
    uuid::Uuid::parse_str(slot_id.trim()).is_ok()
}

/// A fresh slot id.
pub fn new_slot_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

impl PrefabAsset {
    /// Read `<dir>/prefab.json`. A class without one has no components.
    ///
    /// Components that get a slot id assigned here (missing, duplicate, or
    /// not a UUID) are written back to the file once, so every later reader
    /// (the Blueprint editor, other levels) sees the same ids. If the file
    /// is read-only the ids still work for this session, with a warning.
    pub fn load_from_dir(dir: &Path) -> Result<Self, String> {
        let path = dir.join(PREFAB_FILE);
        if !engine_fs::virtual_fs::exists(&path).unwrap_or(false) {
            return Ok(Self::default());
        }
        let bytes = engine_fs::virtual_fs::read_file(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let text = String::from_utf8(bytes).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mut prefab: Self = serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        if prefab.fill_missing_slot_ids() {
            match prefab.save_to_dir(dir) {
                Ok(()) => tracing::info!(path = %path.display(), "Assigned component slot UUIDs"),
                Err(error) => tracing::warn!(
                    path = %path.display(),
                    "Assigned component slot UUIDs but could not save them: {error}"
                ),
            }
        }
        Ok(prefab)
    }

    /// Write `<dir>/prefab.json`.
    pub fn save_to_dir(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(PREFAB_FILE), text).map_err(|e| e.to_string())
    }

    /// Give every component whose slot id is missing, duplicated or not a
    /// UUID a fresh UUID. Returns whether anything changed.
    pub fn fill_missing_slot_ids(&mut self) -> bool {
        let mut used: HashSet<String> = HashSet::new();
        let mut changed = false;
        for component in &mut self.components {
            let id = component.slot_id.trim();
            if !is_slot_uuid(id) || !used.insert(id.to_string()) {
                let fresh = new_slot_id();
                used.insert(fresh.clone());
                component.slot_id = fresh;
                changed = true;
            }
        }
        changed
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_and_readable_slot_ids_become_uuids_once() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(PREFAB_FILE),
            r#"{
                "prefab_version": 1, "name": "Lamp",
                "components": [
                    { "class_name": "LightComponent", "enabled": true, "data": {} },
                    { "slot_id": "LightComponent_1", "class_name": "LightComponent", "enabled": true, "data": {} },
                    { "slot_id": "6f1c1a52-1d7e-4d7e-9c55-2b7c1f0d7a10", "class_name": "StaticMeshComponent", "data": {} }
                ],
                "script_graph": { "nodes": [] }
            }"#,
        )
        .unwrap();
        let first = PrefabAsset::load_from_dir(dir.path()).unwrap();
        let ids: Vec<&str> = first
            .components
            .iter()
            .map(|c| c.slot_id.as_str())
            .collect();
        assert!(ids.iter().all(|id| is_slot_uuid(id)), "{ids:?}");
        assert_eq!(
            ids[2], "6f1c1a52-1d7e-4d7e-9c55-2b7c1f0d7a10",
            "valid UUIDs are kept"
        );
        assert_ne!(ids[0], ids[1]);

        // Saved once: a second load sees the same ids and unknown sections.
        let second = PrefabAsset::load_from_dir(dir.path()).unwrap();
        assert_eq!(
            second
                .components
                .iter()
                .map(|c| c.slot_id.clone())
                .collect::<Vec<_>>(),
            first
                .components
                .iter()
                .map(|c| c.slot_id.clone())
                .collect::<Vec<_>>()
        );
        let text = std::fs::read_to_string(dir.path().join(PREFAB_FILE)).unwrap();
        assert!(text.contains("script_graph"));
    }

    #[test]
    fn duplicate_ids_are_repaired() {
        let id = new_slot_id();
        let mut prefab = PrefabAsset {
            components: vec![
                PrefabComponent {
                    slot_id: id.clone(),
                    class_name: "A".into(),
                    enabled: true,
                    data: Value::Null,
                },
                PrefabComponent {
                    slot_id: id.clone(),
                    class_name: "A".into(),
                    enabled: true,
                    data: Value::Null,
                },
            ],
            ..Default::default()
        };
        assert!(prefab.fill_missing_slot_ids());
        assert_eq!(prefab.components[0].slot_id, id);
        assert_ne!(prefab.components[1].slot_id, id);
        assert!(!prefab.fill_missing_slot_ids(), "idempotent");
    }
}
