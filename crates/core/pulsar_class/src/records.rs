//! Component-record migrations at level load (Pulsar-Native#1035, Phase 3).
//!
//! Older levels store component data in shapes the current classes do not
//! decode, or carry copies of component values in an object's `props`.
//! These migrations bring the raw level JSON up to date before anything is
//! decoded, explicitly and in order, and say what they changed
//! ([`RecordMigrations`]); both the editor and the runtime loader run them
//! through [`crate::migrate::migrate_level_value`]:
//!
//! 1. `MaterialOverrideComponent` (retired) folds into the object's
//!    `StaticMeshComponent` as its one-load `legacy_material_override`.
//! 2. Data written as flat reflected properties for a class whose
//!    serialized shape nests them in `#[sub_props]` groups (a light saved as
//!    `{"intensity": 1000, ...}`) is rewritten to the class's shape.
//! 3. A bare `props.mesh_asset` on an object with no component list becomes
//!    a `StaticMeshComponent` naming that asset.
//! 4. Props a registered scene-props projector manages (copies of component
//!    values the editor used to fold into `props`, and save) are removed:
//!    component values live only in their instances.
//! 5. Fields a class retired ([`RETIRED_FIELDS`]) are removed from its
//!    data, so a saved value no longer carries data nothing reads.
//!
//! Data that still does not decode after this is invalid for its class; the
//! loaders report it (the editor keeps it as an unresolved payload, the
//! runtime refuses the level).

use std::collections::HashMap;

use serde_json::Value;

use crate::overrides::normalize_component_data;
use crate::template::layout;

/// A plugin's migration of its classes' legacy records, run with the
/// others on every level load ([`migrate_component_records`]): the meshes
/// fold the retired `MaterialOverrideComponent` into the mesh and turn a
/// bare `props.mesh_asset` into a mesh component. The plugin's component
/// crate submits one (`inventory::submit!`); this crate names no class.
/// How others do it: Unreal's per-class `Serialize`/`PostLoad` version
/// fix-ups and core redirects, registered by the module owning the class.
pub struct RecordMigrationRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`): skipped
    /// while that package's plugin is disabled.
    pub origin: &'static str,
    /// What it does, for reports ("folded MaterialOverrideComponent into
    /// the mesh").
    pub description: &'static str,
    /// Rewrite one object's component entries (`{class_name, enabled,
    /// data}`) in place; returns whether it changed them.
    pub entries: Option<fn(entries: &mut Vec<Value>) -> bool>,
    /// For an object with no component list: the entry its legacy props
    /// describe, if any.
    pub from_props: Option<fn(object: &Value) -> Option<Value>>,
    /// Props keys the migration retired: removed from every object's props.
    pub stale_props: &'static [&'static str],
}

inventory::collect!(RecordMigrationRegistration);

impl pulsar_plugins::Registration for RecordMigrationRegistration {
    fn origin(&self) -> &'static str {
        self.origin
    }
}

/// Fields classes no longer have: `(class, field)`.
pub const RETIRED_FIELDS: &[(&str, &str)] = &[
    // Water takes its sun from the level's directional light (#1065).
    ("WaterVolumeComponent", "sun_direction"),
];

/// What the record migrations changed, per object id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordMigrations {
    /// `(object id, what)` a plugin's migration changed
    /// ([`RecordMigrationRegistration::description`]).
    pub plugins: Vec<(String, &'static str)>,
    /// `(object id, class)` whose flat data was nested into the class shape.
    pub nested: Vec<(String, String)>,
    /// `(object id, keys)` of projected component values removed from props.
    pub stripped_props: Vec<(String, Vec<String>)>,
    /// `(object id, class, fields)` of retired fields removed from data.
    pub retired_fields: Vec<(String, String, Vec<String>)>,
}

impl RecordMigrations {
    pub fn changed(&self) -> bool {
        !self.plugins.is_empty()
            || !self.nested.is_empty()
            || !self.stripped_props.is_empty()
            || !self.retired_fields.is_empty()
    }
}

/// Run every plugin's entry migration on one component list (a level
/// object's or a class prefab's): what each that changed it did.
pub fn migrate_entries(entries: &mut Vec<Value>) -> Vec<&'static str> {
    pulsar_plugins::registered::<RecordMigrationRegistration>()
        .filter(|migration| migration.entries.is_some_and(|migrate| migrate(entries)))
        .map(|migration| migration.description)
        .collect()
}

/// Run every record migration on `root`, a level's JSON.
pub fn migrate_component_records(root: &mut Value) -> RecordMigrations {
    let mut report = RecordMigrations::default();
    let count = root
        .get("objects")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    for index in 0..count {
        let id = root["objects"][index]
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let lists = crate::migrate::component_lists_of(root, index, &id);
        for list in &lists {
            let Some(entries) = crate::migrate::component_list_mut(root, list) else {
                continue;
            };
            for migration in pulsar_plugins::registered::<RecordMigrationRegistration>() {
                let changed = migration.entries.is_some_and(|migrate| migrate(entries));
                let entry = (id.clone(), migration.description);
                if changed && !report.plugins.contains(&entry) {
                    report.plugins.push(entry);
                }
            }
            for entry in entries.iter_mut() {
                if let Some(class) = nest_flat_properties(entry) {
                    report.nested.push((id.clone(), class));
                }
                if let Some((class, fields)) = strip_retired_fields(entry) {
                    report.retired_fields.push((id.clone(), class, fields));
                }
            }
        }

        let object = &mut root["objects"][index];
        if lists.is_empty() {
            let made: Vec<(Value, &'static str)> = pulsar_plugins::registered::<
                RecordMigrationRegistration,
            >()
            .filter_map(|migration| Some((migration.from_props?(object)?, migration.description)))
            .collect();
            if !made.is_empty() {
                let (entries, descriptions): (Vec<Value>, Vec<&'static str>) =
                    made.into_iter().unzip();
                object
                    .as_object_mut()
                    .expect("level objects are JSON objects")
                    .insert("component_instances".into(), Value::Array(entries));
                for description in descriptions {
                    report.plugins.push((id.clone(), description));
                }
            }
        }

        // Projected copies, for every class the object's authoritative list
        // holds (or the mesh just created).
        let classes: Vec<String> = match crate::migrate::component_lists_of(root, index, &id)
            .first()
            .and_then(|list| crate::migrate::component_list_mut(root, list))
        {
            Some(entries) => entries
                .iter()
                .filter_map(|e| e.get("class_name").and_then(Value::as_str))
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        };
        let removed = strip_projected_props(&mut root["objects"][index], &classes);
        if !removed.is_empty() {
            report.stripped_props.push((id, removed));
        }
    }
    report
}

/// Migration 2. Returns the class when `entry`'s data was rewritten.
fn nest_flat_properties(entry: &mut Value) -> Option<String> {
    let class = entry.get("class_name")?.as_str()?.to_string();
    let data = entry.get("data")?;
    let map = data.as_object()?;
    let layout = layout(&class);
    let flat = map.iter().any(|(key, value)| {
        layout
            .path_of(key)
            .is_some_and(|path| path.len() == 2 && !(value.is_object() && layout.is_group(key)))
    });
    if !flat {
        return None;
    }
    let nested = normalize_component_data(&class, data);
    entry["data"] = nested;
    Some(class)
}

/// Migration 5. Returns the class and the fields removed from `entry`'s
/// data, if any.
fn strip_retired_fields(entry: &mut Value) -> Option<(String, Vec<String>)> {
    let class = entry.get("class_name")?.as_str()?.to_string();
    let data = entry.get_mut("data")?.as_object_mut()?;
    let removed: Vec<String> = RETIRED_FIELDS
        .iter()
        .filter(|(retired_class, field)| *retired_class == class && data.remove(*field).is_some())
        .map(|(_, field)| field.to_string())
        .collect();
    (!removed.is_empty()).then_some((class, removed))
}

/// Migration 4. Removes from `object`'s props every key a projector of one
/// of `classes` manages (asked to clear itself), plus the props keys the
/// plugins' migrations retired.
/// Returns the removed keys.
fn strip_projected_props(object: &mut Value, classes: &[String]) -> Vec<String> {
    let Some(props) = object.get_mut("props").and_then(Value::as_object_mut) else {
        return Vec::new();
    };
    let mut managed: HashMap<String, Value> =
        props.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    for class in classes {
        pulsar_reflection::apply_scene_props_for_class(class, &mut managed, None);
    }
    for migration in pulsar_plugins::registered::<RecordMigrationRegistration>() {
        for key in migration.stale_props {
            managed.remove(*key);
        }
    }
    let mut removed: Vec<String> = props
        .keys()
        .filter(|key| !managed.contains_key(*key))
        .cloned()
        .collect();
    removed.sort();
    props.retain(|key, _| managed.contains_key(key));
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retired_fields_leave_the_data() {
        let mut root = json!({
            "objects": [{ "id": "a", "props": {}, "component_instances": [
                { "class_name": "WaterVolumeComponent",
                  "data": { "size": [1.0, 1.0, 1.0], "sun_direction": [0.5, 1.0, 0.5] } }
            ]}]
        });
        let report = migrate_component_records(&mut root);
        assert_eq!(
            report.retired_fields,
            [(
                "a".to_string(),
                "WaterVolumeComponent".to_string(),
                vec!["sun_direction".to_string()]
            )]
        );
        assert_eq!(
            root["objects"][0]["component_instances"][0]["data"],
            json!({ "size": [1.0, 1.0, 1.0] })
        );
        assert!(!migrate_component_records(&mut root).changed());
    }
}
