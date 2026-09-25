//! Level migration to `ClassInstance` (Pulsar-Native#921).
//!
//! Older levels tie objects to classes two ways, both retired:
//!
//! - a `ScriptComponent { script_asset: "<class dir>" }` on the object
//!   (what dropping a Blueprint into the viewport used to create), or a flat
//!   `props.script_asset` on a `Blueprint` object;
//! - the level's `blueprint_bindings` section: object StableId → classes,
//!   with variable overrides.
//!
//! [`migrate_level_value`] rewrites a level's JSON so those become a
//! `ClassInstance` on the object: script paths are resolved to the class
//! GUID, binding overrides become `variable_overrides`. It works on raw JSON
//! so the editor, the runtime loader and `level_migrate` share it and the
//! old fields keep being *read*. What cannot be expressed as one
//! `ClassInstance` per object (a second class bound to the same object) is
//! left in `blueprint_bindings`, so nothing is lost.

use serde_json::{Map, Value};

use crate::component::ClassInstance;
use crate::registry::ClassRegistry;
use crate::CLASS_INSTANCE;

const SCRIPT_COMPONENT: &str = "ScriptComponent";

/// What a migration changed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// `(object id, class name)` for every `ScriptComponent` converted.
    pub script_components: Vec<(String, String)>,
    /// `(object id, actor type)` for every rust-mode `ScriptComponent`
    /// converted to a `NativeScriptComponent`.
    pub native_scripts: Vec<(String, String)>,
    /// `(object id, class name)` for every binding converted.
    pub bindings: Vec<(String, String)>,
    /// `(object id, class name)` converted without a resolvable class (kept
    /// as an unresolved `ClassInstance` naming the class).
    pub unresolved: Vec<(String, String)>,
    /// `(object id, class name)` bindings left in `blueprint_bindings`.
    pub kept_bindings: Vec<(String, String)>,
}

impl MigrationReport {
    pub fn changed(&self) -> bool {
        !self.script_components.is_empty()
            || !self.bindings.is_empty()
            || !self.native_scripts.is_empty()
    }
}

/// Which component list of an object is authoritative: the level's
/// top-level `components[<id>]` entry when present, else the object's own
/// `component_instances` (or the legacy `props.__component_instances`).
enum ListRef {
    TopLevel(String),
    Inline(usize),
    LegacyProps(usize),
}

fn component_list<'a>(root: &'a mut Value, list: &ListRef) -> Option<&'a mut Vec<Value>> {
    match list {
        ListRef::TopLevel(id) => root.get_mut("components")?.get_mut(id)?.as_array_mut(),
        ListRef::Inline(i) => root
            .get_mut("objects")?
            .get_mut(*i)?
            .get_mut("component_instances")?
            .as_array_mut(),
        ListRef::LegacyProps(i) => root
            .get_mut("objects")?
            .get_mut(*i)?
            .get_mut("props")?
            .get_mut("__component_instances")?
            .as_array_mut(),
    }
}

/// Every component list of object `index` (all of them get converted, so a
/// stale copy never resurrects a `ScriptComponent`), authoritative first.
fn lists_of(root: &Value, index: usize, id: &str) -> Vec<ListRef> {
    let mut lists = Vec::new();
    if root
        .get("components")
        .and_then(|c| c.get(id))
        .and_then(Value::as_array)
        .is_some()
    {
        lists.push(ListRef::TopLevel(id.to_string()));
    }
    let object = &root["objects"][index];
    if object
        .get("component_instances")
        .and_then(Value::as_array)
        .is_some()
    {
        lists.push(ListRef::Inline(index));
    }
    if object
        .get("props")
        .and_then(|p| p.get("__component_instances"))
        .and_then(Value::as_array)
        .is_some()
    {
        lists.push(ListRef::LegacyProps(index));
    }
    lists
}

/// The authoritative list of object `index`, created (as the object's
/// `component_instances`) when it has none.
fn authoritative_list<'a>(root: &'a mut Value, index: usize, id: &str) -> &'a mut Vec<Value> {
    let list = lists_of(root, index, id).into_iter().next();
    match list {
        Some(list) => component_list(root, &list).expect("list exists"),
        None => {
            let object = root["objects"][index].as_object_mut().expect("object");
            object.insert("component_instances".into(), Value::Array(Vec::new()));
            object["component_instances"].as_array_mut().expect("array")
        }
    }
}

fn class_name_of(entry: &Value) -> Option<&str> {
    entry.get("class_name").and_then(Value::as_str)
}

fn class_instance_entry(instance: &ClassInstance, like: Option<&Value>, index: usize) -> Value {
    let mut entry = Map::new();
    if let Some(i) = like.and_then(|l| l.get("index")).cloned() {
        entry.insert("index".into(), i);
    } else if like.is_none() {
        entry.insert("index".into(), Value::from(index));
    }
    entry.insert("class_name".into(), Value::String(CLASS_INSTANCE.into()));
    entry.insert(
        "enabled".into(),
        like.and_then(|l| l.get("enabled"))
            .cloned()
            .unwrap_or(Value::Bool(true)),
    );
    entry.insert("data".into(), instance.to_value());
    Value::Object(entry)
}

fn instance_for_class_name(registry: &ClassRegistry, name: &str) -> (ClassInstance, bool) {
    match registry.by_name(name) {
        Some(entry) => (
            ClassInstance::new(entry.id.clone(), entry.name.clone()),
            true,
        ),
        None => (ClassInstance::new(Default::default(), name), false),
    }
}

/// Migrate a level's JSON in place. See the module doc.
pub fn migrate_level_value(root: &mut Value, registry: &ClassRegistry) -> MigrationReport {
    let mut report = MigrationReport::default();
    let object_ids: Vec<String> = root
        .get("objects")
        .and_then(Value::as_array)
        .map(|objects| {
            objects
                .iter()
                .map(|o| {
                    o.get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();

    // ── ScriptComponent → ClassInstance ────────────────────────────────
    for (index, id) in object_ids.iter().enumerate() {
        let lists = lists_of(root, index, id);
        let mut converted_class: Option<String> = None;
        for list in &lists {
            let Some(entries) = component_list(root, list) else {
                continue;
            };
            let has_instance = entries
                .iter()
                .any(|e| class_name_of(e) == Some(CLASS_INSTANCE));
            let mut placed_instance = has_instance;
            let mut i = 0;
            while i < entries.len() {
                let entry = &entries[i];
                if class_name_of(entry) != Some(SCRIPT_COMPONENT) {
                    i += 1;
                    continue;
                }
                // Rust-mode binding → NativeScriptComponent.
                if entry
                    .get("data")
                    .and_then(|d| d.get("mode"))
                    .and_then(Value::as_str)
                    == Some("rust")
                {
                    let data = entry.get("data").cloned().unwrap_or_default();
                    let field = |k: &str| {
                        data.get(k)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    let native = crate::NativeScriptComponent::new(
                        field("script_crate"),
                        field("actor_type"),
                    );
                    let mut converted = Map::new();
                    if let Some(index) = entry.get("index").cloned() {
                        converted.insert("index".into(), index);
                    }
                    converted.insert(
                        "class_name".into(),
                        Value::String(crate::native_script::NATIVE_SCRIPT_COMPONENT.into()),
                    );
                    converted.insert(
                        "enabled".into(),
                        entry.get("enabled").cloned().unwrap_or(Value::Bool(true)),
                    );
                    converted.insert(
                        "data".into(),
                        serde_json::to_value(&native).unwrap_or_default(),
                    );
                    report
                        .native_scripts
                        .push((id.clone(), native.actor_type.clone()));
                    entries[i] = Value::Object(converted);
                    i += 1;
                    continue;
                }
                let path = entry
                    .get("data")
                    .and_then(|d| d.get("script_asset"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Some(class) = registry.resolve_script_asset(path) else {
                    i += 1;
                    continue;
                };
                if placed_instance {
                    if converted_class.as_deref().is_some_and(|c| c != class.name) {
                        // A second class on one object: keep it as it was.
                        tracing::warn!(object = %id, class = %class.name, "Object already has a class; keeping its extra ScriptComponent");
                        i += 1;
                        continue;
                    }
                    entries.remove(i);
                } else {
                    let instance = ClassInstance::new(class.id.clone(), class.name.clone());
                    entries[i] = class_instance_entry(&instance, Some(&entries[i].clone()), i);
                    placed_instance = true;
                    i += 1;
                }
                if converted_class.is_none() {
                    converted_class = Some(class.name.clone());
                    report
                        .script_components
                        .push((id.clone(), class.name.clone()));
                }
            }
        }

        // Flat `props.script_asset` on a Blueprint object (oldest files).
        let object = &root["objects"][index];
        let is_blueprint = object.get("object_type").and_then(Value::as_str) == Some("Blueprint");
        let flat = object
            .get("props")
            .and_then(|p| p.get("script_asset"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(flat) = flat {
            if let Some(class) = registry.resolve_script_asset(&flat).cloned() {
                if converted_class.is_none() && is_blueprint {
                    let list = authoritative_list(root, index, id);
                    if !list
                        .iter()
                        .any(|e| class_name_of(e) == Some(CLASS_INSTANCE))
                    {
                        let instance = ClassInstance::new(class.id.clone(), class.name.clone());
                        let len = list.len();
                        list.insert(0, class_instance_entry(&instance, None, len));
                        report
                            .script_components
                            .push((id.clone(), class.name.clone()));
                    }
                }
                if let Some(props) = root["objects"][index]
                    .get_mut("props")
                    .and_then(Value::as_object_mut)
                {
                    props.remove("script_asset");
                }
            }
        }
    }

    // ── blueprint_bindings → ClassInstance ─────────────────────────────
    let bindings = root
        .as_object_mut()
        .and_then(|r| r.remove("blueprint_bindings"))
        .and_then(|b| match b {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default();
    let mut kept = Map::new();
    for (stable_id, entries) in bindings {
        let Some(index) = object_ids.iter().position(|id| *id == stable_id) else {
            kept.insert(stable_id, entries);
            continue;
        };
        let mut left = Vec::new();
        for binding in entries.as_array().cloned().unwrap_or_default() {
            let class_name = binding
                .get("class_name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let overrides: Map<String, Value> = binding
                .get("overrides")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            let (fresh, resolved) = instance_for_class_name(registry, &class_name);
            let list = authoritative_list(root, index, &stable_id);
            let existing = list
                .iter_mut()
                .find(|e| class_name_of(e) == Some(CLASS_INSTANCE));
            match existing {
                Some(entry) => {
                    let mut instance = entry
                        .get("data")
                        .and_then(ClassInstance::from_json)
                        .unwrap_or_default();
                    let same = (!fresh.class.is_empty() && instance.class == fresh.class)
                        || (instance.class_name == class_name && !class_name.is_empty());
                    if !same {
                        report
                            .kept_bindings
                            .push((stable_id.clone(), class_name.clone()));
                        left.push(binding);
                        continue;
                    }
                    for (k, v) in overrides {
                        instance.variable_overrides.entry(k).or_insert(v);
                    }
                    if let Some(map) = entry.as_object_mut() {
                        map.insert("data".into(), instance.to_value());
                    }
                }
                None => {
                    let mut instance = fresh;
                    instance.variable_overrides = overrides.into_iter().collect();
                    let len = list.len();
                    list.insert(0, class_instance_entry(&instance, None, len));
                }
            }
            if !resolved {
                report
                    .unresolved
                    .push((stable_id.clone(), class_name.clone()));
            }
            report.bindings.push((stable_id.clone(), class_name));
        }
        if !left.is_empty() {
            kept.insert(stable_id, Value::Array(left));
        }
    }
    if !kept.is_empty() {
        if let Some(map) = root.as_object_mut() {
            map.insert("blueprint_bindings".into(), Value::Object(kept));
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::ClassId;
    use crate::registry::ClassEntry;
    use serde_json::json;

    fn registry() -> ClassRegistry {
        ClassRegistry::from_entries(vec![
            ClassEntry {
                id: ClassId::from("guid-lamp"),
                name: "Lamp".into(),
                dir: "/p/src/classes/Lamp".into(),
            },
            ClassEntry {
                id: ClassId::from("guid-door"),
                name: "Door".into(),
                dir: "/p/src/classes/Door".into(),
            },
        ])
    }

    #[test]
    fn script_components_become_class_instances() {
        let mut level = json!({
            "version": "2.1",
            "objects": [
                { "id": "a", "name": "Lamp", "object_type": "Blueprint", "props": {},
                  "component_instances": [
                      { "index": 0, "class_name": "ScriptComponent", "data": { "script_asset": "C:/old/machine/src/classes/Lamp" } }
                  ] },
                { "id": "b", "name": "Other", "object_type": "Empty", "props": {} },
                { "id": "c", "name": "Legacy", "object_type": "Blueprint", "props": { "script_asset": "/x/src/classes/Door" } }
            ],
            "components": {
                "a": [
                    { "class_name": "ScriptComponent", "enabled": true, "data": { "script_asset": "C:/old/machine/src/classes/Lamp" } },
                    { "class_name": "LightComponent", "enabled": true, "data": {} }
                ],
                "b": [
                    { "class_name": "ScriptComponent", "enabled": true, "data": { "script_asset": "/somewhere/NotAClass" } },
                    { "class_name": "ScriptComponent", "enabled": true,
                      "data": { "mode": "rust", "script_crate": "game_scripts", "actor_type": "Spinner" } }
                ]
            }
        });
        let report = migrate_level_value(&mut level, &registry());
        assert!(report.changed());
        let a = &level["components"]["a"];
        assert_eq!(a[0]["class_name"], "ClassInstance");
        assert_eq!(a[0]["data"]["class"], "guid-lamp");
        assert_eq!(a[1]["class_name"], "LightComponent");
        assert_eq!(
            level["objects"][0]["component_instances"][0]["class_name"],
            "ClassInstance"
        );
        // Not a class: untouched.
        assert_eq!(level["components"]["b"][0]["class_name"], "ScriptComponent");
        // Rust-mode binding: its own component.
        let native = &level["components"]["b"][1];
        assert_eq!(native["class_name"], "NativeScriptComponent");
        assert_eq!(
            native["data"],
            json!({ "script_crate": "game_scripts", "actor_type": "Spinner" })
        );
        // Flat legacy prop.
        assert_eq!(
            level["objects"][2]["component_instances"][0]["data"]["class"],
            "guid-door"
        );
        assert!(level["objects"][2]["props"].get("script_asset").is_none());
        // Idempotent.
        let before = level.clone();
        assert!(!migrate_level_value(&mut level, &registry()).changed());
        assert_eq!(level, before);
    }

    #[test]
    fn bindings_become_class_instances_with_variable_overrides() {
        let mut level = json!({
            "version": "2.1",
            "objects": [
                { "id": "lever", "name": "Lever", "object_type": "Empty", "props": {} },
                { "id": "gone", "name": "Gone", "object_type": "Empty", "props": {} }
            ],
            "components": {},
            "blueprint_bindings": {
                "lever": [
                    { "class_name": "Door", "overrides": { "speed": 7.5 } },
                    { "class_name": "Lamp" }
                ],
                "gone": [ { "class_name": "Missing", "overrides": { "x": 1 } } ],
                "nobody": [ { "class_name": "Lamp" } ]
            }
        });
        let report = migrate_level_value(&mut level, &registry());
        let lever = &level["objects"][0]["component_instances"][0];
        assert_eq!(lever["class_name"], "ClassInstance");
        assert_eq!(lever["data"]["class"], "guid-door");
        assert_eq!(lever["data"]["variable_overrides"]["speed"], 7.5);
        // Unresolved class: kept by name, overrides preserved.
        let gone = &level["objects"][1]["component_instances"][0]["data"];
        assert_eq!(gone["class_name"], "Missing");
        assert_eq!(gone["variable_overrides"]["x"], 1);
        assert_eq!(
            report.unresolved,
            [("gone".to_string(), "Missing".to_string())]
        );
        // Second class on one object and unknown objects stay as bindings.
        let kept = &level["blueprint_bindings"];
        assert_eq!(kept["lever"][0]["class_name"], "Lamp");
        assert!(kept.get("nobody").is_some());
    }
}
