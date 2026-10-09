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

use serde_json::{Map, Value};

use crate::overrides::normalize_component_data;
use crate::template::layout;

const MATERIAL_OVERRIDE: &str = "MaterialOverrideComponent";
const STATIC_MESH: &str = "StaticMeshComponent";

/// Fields classes no longer have: `(class, field)`.
pub const RETIRED_FIELDS: &[(&str, &str)] = &[
    // Water takes its sun from the level's directional light (#1065).
    ("WaterVolumeComponent", "sun_direction"),
];

/// What the record migrations changed, per object id.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordMigrations {
    /// Objects whose `MaterialOverrideComponent` was folded into their mesh
    /// (or dropped, with no mesh to take it).
    pub material_overrides: Vec<String>,
    /// `(object id, class)` whose flat data was nested into the class shape.
    pub nested: Vec<(String, String)>,
    /// Objects whose `props.mesh_asset` became a `StaticMeshComponent`.
    pub mesh_asset_props: Vec<String>,
    /// `(object id, keys)` of projected component values removed from props.
    pub stripped_props: Vec<(String, Vec<String>)>,
    /// `(object id, class, fields)` of retired fields removed from data.
    pub retired_fields: Vec<(String, String, Vec<String>)>,
}

impl RecordMigrations {
    pub fn changed(&self) -> bool {
        !self.material_overrides.is_empty()
            || !self.nested.is_empty()
            || !self.mesh_asset_props.is_empty()
            || !self.stripped_props.is_empty()
            || !self.retired_fields.is_empty()
    }
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
            if fold_material_override(entries) && !report.material_overrides.contains(&id) {
                report.material_overrides.push(id.clone());
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
            if let Some(entry) = mesh_from_props(object) {
                object
                    .as_object_mut()
                    .expect("level objects are JSON objects")
                    .insert("component_instances".into(), Value::Array(vec![entry]));
                report.mesh_asset_props.push(id.clone());
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

/// Migration 1. Returns whether the list held a `MaterialOverrideComponent`.
fn fold_material_override(entries: &mut Vec<Value>) -> bool {
    let Some(position) = entries
        .iter()
        .position(|e| e.get("class_name").and_then(Value::as_str) == Some(MATERIAL_OVERRIDE))
    else {
        return false;
    };
    let legacy = entries
        .remove(position)
        .get("data")
        .cloned()
        .unwrap_or(Value::Null);
    let mesh = entries
        .iter_mut()
        .find(|e| e.get("class_name").and_then(Value::as_str) == Some(STATIC_MESH))
        .and_then(|e| e.get_mut("data"))
        .and_then(Value::as_object_mut);
    if let Some(mesh) = mesh {
        mesh.entry("legacy_material_override").or_insert(legacy);
    }
    true
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

/// Migration 3. A `StaticMeshComponent` entry for `object`'s non-empty
/// `props.mesh_asset`, in the class's own shape.
fn mesh_from_props(object: &Value) -> Option<Value> {
    let path = object
        .get("props")?
        .get("mesh_asset")?
        .as_str()
        .filter(|path| !path.trim().is_empty())?;
    let mut data = pulsar_reflection::REGISTRY
        .create_instance(STATIC_MESH)
        .and_then(|instance| instance.to_json().ok())
        .unwrap_or_else(|| Value::Object(Map::new()));
    data.as_object_mut()?
        .insert("mesh_asset".into(), Value::String(path.to_string()));
    Some(serde_json::json!({
        "class_name": STATIC_MESH,
        "enabled": true,
        "data": data,
    }))
}

/// Migration 4. Removes from `object`'s props every key a projector of one
/// of `classes` manages (asked to clear itself), plus a stale `mesh_asset`.
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
    managed.remove("mesh_asset");
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
    fn material_overrides_fold_into_the_mesh() {
        let mut root = json!({
            "objects": [{ "id": "a", "props": {}, "component_instances": [
                { "class_name": "StaticMeshComponent", "data": { "mesh_asset": "m.glb" } },
                { "class_name": "MaterialOverrideComponent", "data": { "roughness": 0.3 } }
            ]}]
        });
        let report = migrate_component_records(&mut root);
        assert_eq!(report.material_overrides, ["a"]);
        let list = root["objects"][0]["component_instances"]
            .as_array()
            .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(
            list[0]["data"]["legacy_material_override"],
            json!({ "roughness": 0.3 })
        );
    }

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

    #[test]
    fn a_bare_mesh_asset_prop_becomes_a_component_and_leaves_props() {
        let mut root = json!({
            "objects": [
                { "id": "a", "props": { "mesh_asset": "m.glb", "icon_asset": "i.png" } },
                { "id": "b", "props": { "mesh_asset": "m.glb" }, "component_instances": [] }
            ]
        });
        let report = migrate_component_records(&mut root);
        assert_eq!(report.mesh_asset_props, ["a"]);
        assert_eq!(
            root["objects"][0]["component_instances"][0]["data"]["mesh_asset"],
            "m.glb"
        );
        assert_eq!(
            root["objects"][0]["props"],
            json!({ "icon_asset": "i.png" })
        );
        // An object with a component list keeps it as it is; the stale
        // copy goes.
        assert_eq!(root["objects"][1]["component_instances"], json!([]));
        assert_eq!(root["objects"][1]["props"], json!({}));
    }
}
