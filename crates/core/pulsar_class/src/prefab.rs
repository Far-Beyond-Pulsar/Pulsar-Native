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
//! `<Class>_<n>` ids of early #921 builds), gets fresh UUIDs on load.
//! [`PrefabAsset::load_from_dir`] migrates in memory and never writes (read
//! paths); [`PrefabAsset::load_and_repair`] and [`PrefabLoad::repair`] save
//! the migrated form so the ids stay stable, and report a failed save.

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

/// What [`PrefabAsset::reassign_slot_ids`] changed.
#[derive(Debug, Default)]
pub struct SlotReassignment {
    pub changed: bool,
    /// Replaced readable id → the UUID that replaced it.
    pub aliases: std::collections::BTreeMap<String, String>,
}

/// A prefab read with its migrations applied in memory, plus what changed.
#[derive(Debug)]
pub struct PrefabLoad {
    pub prefab: PrefabAsset,
    /// Replaced readable slot id -> the UUID that replaced it.
    pub aliases: std::collections::BTreeMap<String, String>,
    /// What each record migration that ran did.
    pub migrated: Vec<&'static str>,
    /// Whether any slot id was assigned or replaced.
    pub slots_changed: bool,
}

impl From<PrefabAsset> for PrefabLoad {
    fn from(prefab: PrefabAsset) -> Self {
        Self {
            prefab,
            aliases: Default::default(),
            migrated: Vec::new(),
            slots_changed: false,
        }
    }
}

impl PrefabLoad {
    /// Whether the file on disk differs from the migrated form.
    pub fn needs_repair(&self) -> bool {
        self.slots_changed || !self.migrated.is_empty()
    }

    /// Save the migrated form: record replaced slot ids in `class.json`
    /// first (so a level keyed by the old ids can still find its slots,
    /// #933), then write `prefab.json`. Both writes go through `engine_fs`;
    /// a failure of either is returned.
    pub fn repair(&self, dir: &Path) -> Result<(), String> {
        if !self.aliases.is_empty() {
            crate::id::ClassMeta::record_slot_aliases(dir, self.aliases.clone())?;
        }
        self.prefab.save_to_dir(dir)
    }
}

/// What [`PrefabAsset::load_and_repair`] did to the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepairOutcome {
    /// Already in migrated form; nothing written.
    Unchanged,
    /// The migrated form was saved.
    Saved,
    /// The migrated form is only in memory: the save failed.
    Failed(String),
}

impl PrefabAsset {
    /// Read `<dir>/prefab.json` and migrate it **in memory**; nothing is
    /// written. A class without a prefab has no components.
    ///
    /// This is the form for read paths (palettes, previews, anything that
    /// only looks): it succeeds on a read-only checkout and leaves the file
    /// byte-for-byte as it was. Components that need a slot id get fresh
    /// UUIDs, which are *not* stable until saved: use
    /// [`Self::load_and_repair`] where they must persist, or keep the
    /// [`PrefabLoad`] from [`Self::read_migrated`] and save it explicitly.
    pub fn load_from_dir(dir: &Path) -> Result<Self, String> {
        Self::read_migrated(dir).map(|load| load.prefab)
    }

    /// [`Self::load_from_dir`], also reporting what the migration changed
    /// (and the replaced slot ids) so the caller can save it explicitly with
    /// [`PrefabLoad::repair`]. Never writes.
    pub fn read_migrated(dir: &Path) -> Result<PrefabLoad, String> {
        let path = dir.join(PREFAB_FILE);
        if !engine_fs::virtual_fs::exists(&path).unwrap_or(false) {
            return Ok(PrefabLoad::from(Self::default()));
        }
        let bytes = engine_fs::virtual_fs::read_file(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let text = String::from_utf8(bytes)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let mut prefab: Self = serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
        // The plugins' record migrations, as a level's objects get them.
        let migrated = prefab.migrate_records();
        for what in &migrated {
            tracing::debug!(path = %path.display(), "{what}");
        }
        let renamed = prefab.reassign_slot_ids();
        Ok(PrefabLoad {
            prefab,
            aliases: renamed.aliases,
            migrated,
            slots_changed: renamed.changed,
        })
    }

    /// Read `<dir>/prefab.json` and, when migrating changed anything (a slot
    /// id assigned, a record migrated), save the migrated form back so every
    /// later reader sees the same ids. For callers that must have stable
    /// slot ids (placing a class in a level, cooking). A failed save does not
    /// fail the load (the ids work for this session) but is returned in
    /// [`RepairOutcome::Failed`] for the caller to report.
    pub fn load_and_repair(dir: &Path) -> Result<(Self, RepairOutcome), String> {
        let load = Self::read_migrated(dir)?;
        let outcome = if load.needs_repair() {
            match load.repair(dir) {
                Ok(()) => {
                    tracing::info!(dir = %dir.display(), "Saved migrated prefab");
                    RepairOutcome::Saved
                }
                Err(error) => {
                    tracing::warn!(
                        dir = %dir.display(),
                        "Migrated the prefab in memory but could not save it: {error}"
                    );
                    RepairOutcome::Failed(error)
                }
            }
        } else {
            RepairOutcome::Unchanged
        };
        Ok((load.prefab, outcome))
    }

    /// Run the plugins' record migrations
    /// ([`crate::records::migrate_entries`]) on the components: what each
    /// that changed them did. A component a migration adds gets its slot id
    /// from [`Self::reassign_slot_ids`].
    pub fn migrate_records(&mut self) -> Vec<&'static str> {
        let Ok(Value::Array(mut entries)) = serde_json::to_value(&self.components) else {
            return Vec::new();
        };
        let migrated = crate::records::migrate_entries(&mut entries);
        if migrated.is_empty() {
            return migrated;
        }
        match serde_json::from_value(Value::Array(entries)) {
            Ok(components) => {
                self.components = components;
                migrated
            }
            Err(error) => {
                tracing::warn!(
                    "a record migration left the prefab unreadable, kept as it was: {error}"
                );
                Vec::new()
            }
        }
    }

    /// Write `<dir>/prefab.json`.
    pub fn save_to_dir(&self, dir: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let path = dir.join(PREFAB_FILE);
        engine_fs::virtual_fs::write_file(&path, text.as_bytes())
            .map_err(|e| format!("failed to write {}: {e}", path.display()))
    }

    /// Give every component whose slot id is missing, duplicated or not a
    /// UUID a fresh UUID. Returns whether anything changed.
    pub fn fill_missing_slot_ids(&mut self) -> bool {
        self.reassign_slot_ids().changed
    }

    /// [`Self::fill_missing_slot_ids`], also reporting which non-empty,
    /// non-UUID ids (the readable `<Class>_<n>` ids) were replaced by what.
    /// A duplicated readable id maps to the first component that had it.
    pub fn reassign_slot_ids(&mut self) -> SlotReassignment {
        let mut used: HashSet<String> = HashSet::new();
        let mut result = SlotReassignment::default();
        for component in &mut self.components {
            let id = component.slot_id.trim().to_string();
            if !is_slot_uuid(&id) || !used.insert(id.clone()) {
                let fresh = new_slot_id();
                used.insert(fresh.clone());
                if !id.is_empty() && !is_slot_uuid(&id) {
                    result.aliases.entry(id).or_insert_with(|| fresh.clone());
                }
                component.slot_id = fresh;
                result.changed = true;
            }
        }
        result
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
        let (first, outcome) = PrefabAsset::load_and_repair(dir.path()).unwrap();
        assert_eq!(outcome, RepairOutcome::Saved);
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
        let (second, outcome) = PrefabAsset::load_and_repair(dir.path()).unwrap();
        assert_eq!(outcome, RepairOutcome::Unchanged);
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

    #[test]
    fn replaced_readable_ids_are_recorded_in_class_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(PREFAB_FILE),
            r#"{ "prefab_version": 1, "name": "Lamp", "components": [
                { "slot_id": "LightComponent_1", "class_name": "LightComponent", "data": {} },
                { "class_name": "StaticMeshComponent", "data": {} }
            ] }"#,
        )
        .unwrap();
        let (prefab, _) = PrefabAsset::load_and_repair(dir.path()).unwrap();
        let aliases = crate::id::ClassMeta::read(dir.path()).unwrap().slot_aliases;
        // Only the readable id is an alias; a missing id had nothing to map.
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases["LightComponent_1"], prefab.components[0].slot_id);
    }

    // A migration only this test module's records trigger: a retired class
    // name becomes its current name.
    fn rename_w6_legacy(entries: &mut Vec<Value>) -> bool {
        let mut changed = false;
        for entry in entries {
            if entry["class_name"] == "W6LegacyComponent" {
                entry["class_name"] = Value::String("W6Component".into());
                changed = true;
            }
        }
        changed
    }

    inventory::submit! {
        crate::records::RecordMigrationRegistration {
            origin: pulsar_plugins::origin!(),
            name: "pulsar_class.tests.w6_rename",
            description: "renamed W6LegacyComponent",
            entries: Some(rename_w6_legacy),
            from_props: None,
            stale_props: &[],
            retired_fields: &[],
        }
    }

    const LEGACY: &str = r#"{
        "prefab_version": 1, "name": "Lamp",
        "components": [
            { "class_name": "W6LegacyComponent", "data": {} },
            { "slot_id": "W6Component_1", "class_name": "W6Component", "data": {} }
        ],
        "script_graph": { "nodes": [] }
    }"#;

    /// A class dir whose prefab (and, on unix, the dir) cannot be written.
    fn read_only_class_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PREFAB_FILE);
        std::fs::write(&path, LEGACY).unwrap();
        set_read_only(dir.path(), &path, true);
        dir
    }

    fn set_read_only(dir: &Path, file: &Path, on: bool) {
        let mut perms = std::fs::metadata(file).unwrap().permissions();
        perms.set_readonly(on);
        std::fs::set_permissions(file, perms).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = if on { 0o555 } else { 0o755 };
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let _ = dir;
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn pure_load_of_a_read_only_class_migrates_in_memory_and_writes_nothing() {
        let dir = read_only_class_dir();
        let path = dir.path().join(PREFAB_FILE);
        let before = std::fs::read(&path).unwrap();
        let listing = entries(dir.path());

        let prefab = PrefabAsset::load_from_dir(dir.path()).unwrap();
        assert!(prefab.components.iter().all(|c| is_slot_uuid(&c.slot_id)));
        assert!(
            prefab
                .components
                .iter()
                .all(|c| c.class_name == "W6Component"),
            "the record migration ran in memory"
        );
        assert!(prefab.extra.contains_key("script_graph"));

        assert_eq!(std::fs::read(&path).unwrap(), before, "file byte-for-byte");
        assert_eq!(entries(dir.path()), listing, "no class.json or temp file");
        set_read_only(dir.path(), &path, false);
    }

    #[test]
    fn load_and_repair_reports_a_failed_save_and_still_loads() {
        let dir = read_only_class_dir();
        let path = dir.path().join(PREFAB_FILE);
        let before = std::fs::read(&path).unwrap();

        let (prefab, outcome) = PrefabAsset::load_and_repair(dir.path()).unwrap();
        assert!(prefab.components.iter().all(|c| is_slot_uuid(&c.slot_id)));
        assert!(matches!(outcome, RepairOutcome::Failed(_)), "{outcome:?}");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        set_read_only(dir.path(), &path, false);
    }

    #[test]
    fn explicit_repair_writes_the_migrated_form_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(PREFAB_FILE);
        std::fs::write(&path, LEGACY).unwrap();

        // Reading changes nothing; saving is a separate, explicit step.
        let load = PrefabAsset::read_migrated(dir.path()).unwrap();
        assert!(load.needs_repair());
        assert_eq!(load.migrated, vec!["renamed W6LegacyComponent"]);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), LEGACY);
        load.repair(dir.path()).unwrap();

        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["components"][0]["class_name"], "W6Component");
        assert_eq!(
            saved["components"][0]["slot_id"],
            load.prefab.components[0].slot_id.as_str()
        );
        assert!(saved["script_graph"].is_object());
        let aliases = crate::id::ClassMeta::read(dir.path()).unwrap().slot_aliases;
        assert_eq!(aliases["W6Component_1"], load.prefab.components[1].slot_id);

        // Migrated once: reading again changes nothing, byte for byte.
        let again = PrefabAsset::read_migrated(dir.path()).unwrap();
        assert!(!again.needs_repair());
        assert_eq!(again.prefab.components, load.prefab.components);
        let bytes = std::fs::read(&path).unwrap();
        let (_, outcome) = PrefabAsset::load_and_repair(dir.path()).unwrap();
        assert_eq!(outcome, RepairOutcome::Unchanged);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}
