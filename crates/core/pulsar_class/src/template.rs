//! Typed class templates (Pulsar-Native#1035, Phase 3).
//!
//! A [`ClassTemplate`] is a class definition with every prefab slot's
//! default decoded once into a typed value. Instances are built by cloning
//! a slot's value and applying the instance's overrides through the class's
//! reflected setters, so placing, loading, rebuilding or spawning an
//! instance decodes nothing and loads no asset the class default already
//! loaded (a mesh slot's geometry is read once per class, not once per
//! instance).
//!
//! Overrides keep their persisted form, a JSON diff against the slot
//! default ([`crate::overrides`]); the file is the boundary. A diff is
//! mapped onto reflected properties by where each property sits in the
//! class's serialized shape ([`PropertyLayout`]): a top-level field, or a
//! field one `#[sub_props]` group down. A diff that names something no
//! reflected property covers (a serde-only field) is applied the old way,
//! by decoding the patched default, so nothing an override says is lost.
//!
//! Templates are cached per class and rebuilt when the definition changes
//! ([`template`]).

use std::any::Any;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use pulsar_scene_model::attachments::UnresolvedComponent;
use pulsar_world_registry::InstanceValue;
use serde_json::{Map, Value};

use crate::id::ClassId;
use crate::overrides::{apply, is_meta_key, normalize_component_data, split_meta, values_equal};
use crate::registry::ClassDefinition;

/// One prefab slot's class default.
pub struct SlotTemplate {
    pub class_name: String,
    /// The default normalized to the component's serialized shape, without
    /// metadata: what override diffs are taken against.
    pub default_data: Value,
    /// The default as a typed value, or why there is none (a class this
    /// build does not register, or data that does not decode).
    pub value: Result<Box<dyn Any + Send + Sync>, String>,
}

/// A class definition with its slot defaults decoded.
pub struct ClassTemplate {
    pub def: ClassDefinition,
    slots: HashMap<String, SlotTemplate>,
}

impl ClassTemplate {
    /// Decode every slot default of `def` (once).
    pub fn build(def: ClassDefinition) -> Self {
        let slots = def
            .prefab
            .components
            .iter()
            .map(|component| {
                let normalized = normalize_component_data(&component.class_name, &component.data);
                let default_data = split_meta(&normalized).1;
                let value = match pulsar_world_registry::decode_world_component_value(
                    &component.class_name,
                    &default_data,
                ) {
                    Some(Ok(value)) => Ok(value),
                    Some(Err(error)) => Err(format!(
                        "`{}` class default does not decode: {error}",
                        component.class_name
                    )),
                    None => Err(format!(
                        "`{}` is not a registered component class",
                        component.class_name
                    )),
                };
                if let Err(reason) = &value {
                    tracing::warn!(class = %def.name, slot = %component.slot_id, "{reason}");
                }
                (
                    component.slot_id.clone(),
                    SlotTemplate {
                        class_name: component.class_name.clone(),
                        default_data,
                        value,
                    },
                )
            })
            .collect();
        Self { def, slots }
    }

    pub fn slot(&self, slot_id: &str) -> Option<&SlotTemplate> {
        self.slots.get(slot_id)
    }

    /// The value an instance's component for `slot_id` starts with: the
    /// slot default with `overrides` (the instance's diff for the slot, if
    /// any) applied. Unresolved when the class default has no typed value;
    /// the payload then is the patched default data.
    pub fn slot_value(&self, slot_id: &str, overrides: Option<&Value>) -> Option<InstanceValue> {
        let slot = self.slots.get(slot_id)?;
        let unresolved = |reason: &str| {
            let mut data = slot.default_data.clone();
            if let Some(patch) = overrides {
                apply(&mut data, patch);
            }
            InstanceValue::Unresolved(UnresolvedComponent {
                data,
                reason: reason.to_string(),
            })
        };
        let default = match &slot.value {
            Ok(default) => default,
            Err(reason) => return Some(unresolved(reason)),
        };
        let class = slot.class_name.as_str();
        let registration_clone =
            |value: &(dyn Any + Send + Sync)| pulsar_world_registry::clone_value(class, value);
        let Some(mut value) = registration_clone(default.as_ref()) else {
            return Some(unresolved("the class default could not be cloned"));
        };
        let Some(patch) = overrides.filter(|patch| has_values(patch)) else {
            return Some(InstanceValue::Value(value));
        };
        if apply_diff(class, value.as_mut(), patch) {
            return Some(InstanceValue::Value(value));
        }
        // A diff reflection cannot place: decode the patched default, as
        // building an instance always did before.
        let mut data = slot.default_data.clone();
        apply(&mut data, patch);
        Some(
            match pulsar_world_registry::decode_world_component_value(class, &data) {
                Some(Ok(value)) => InstanceValue::Value(value),
                Some(Err(error)) => unresolved(&format!(
                    "overridden `{class}` data does not decode: {error}"
                )),
                None => unresolved("not a registered component class"),
            },
        )
    }

    /// The class default of reflected property `property` of slot `slot_id`
    /// (typed, as the property's getter returns it).
    pub fn default_property(&self, slot_id: &str, property: &str) -> Option<Box<dyn Any>> {
        let slot = self.slots.get(slot_id)?;
        let value = slot.value.as_ref().ok()?;
        let instance = pulsar_world_registry::value_engine_class(&slot.class_name, value.as_ref())?;
        instance
            .get_properties()
            .into_iter()
            .find(|p| p.name == property)
            .map(|p| (p.getter)(instance))
    }
}

/// Whether a diff sets anything (metadata keys aside).
fn has_values(patch: &Value) -> bool {
    match patch.as_object() {
        Some(map) => map.keys().any(|key| !is_meta_key(key)),
        None => true,
    }
}

/// Apply override diff `patch` to `value` (a `class` value) property by
/// property through reflection. `false` when some part of the diff maps to
/// no reflected property or does not convert (the value may then be
/// partly written; the caller discards it and decodes the patched default
/// instead).
pub fn apply_diff(class: &str, value: &mut (dyn Any + Send + Sync), patch: &Value) -> bool {
    let Some(map) = patch.as_object() else {
        return false;
    };
    let layout = layout(class);
    let mut edits: Vec<(&'static str, &Value)> = Vec::new();
    for (key, sub) in map.iter().filter(|(key, _)| !is_meta_key(key)) {
        if let Some(property) = layout.at(&[key.as_str()]) {
            edits.push((property, sub));
            continue;
        }
        let Some(group) = sub.as_object().filter(|_| layout.is_group(key)) else {
            return false;
        };
        for (field, sub) in group.iter().filter(|(field, _)| !is_meta_key(field)) {
            match layout.at(&[key.as_str(), field.as_str()]) {
                Some(property) => edits.push((property, sub)),
                None => return false,
            }
        }
    }
    for (property, sub) in edits {
        let Some(instance) = pulsar_world_registry::value_engine_class(class, &*value) else {
            return false;
        };
        let Some(metadata) = instance
            .get_properties()
            .into_iter()
            .find(|p| p.name == property)
        else {
            return false;
        };
        let Ok(mut current) =
            RUNTIME_TYPE_REGISTRY.serialize_json_for_any((metadata.getter)(instance).as_ref())
        else {
            return false;
        };
        apply(&mut current, sub);
        let Ok(typed) =
            RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(metadata.type_info, current)
        else {
            return false;
        };
        if pulsar_world_registry::set_value_property(class, value, property, typed).is_err() {
            return false;
        }
    }
    true
}

// ── Property layout ────────────────────────────────────────────────────────

/// Where each reflected property of a class sits in the class's serialized
/// (`to_json`) shape: at the top level, or inside a `#[sub_props]` group.
pub struct PropertyLayout {
    paths: Vec<(&'static str, Vec<String>)>,
}

impl PropertyLayout {
    fn compute(class: &str) -> Self {
        let Some(instance) = REGISTRY.create_instance(class) else {
            return Self { paths: Vec::new() };
        };
        let shape = instance.to_json().unwrap_or(Value::Null);
        let shape = shape.as_object().cloned().unwrap_or_default();
        let paths = instance
            .get_properties()
            .into_iter()
            .map(|prop| {
                let value = RUNTIME_TYPE_REGISTRY
                    .serialize_json_for_any((prop.getter)(instance.as_ref()).as_ref())
                    .ok();
                (prop.name, locate(&shape, prop.name, value.as_ref()))
            })
            .collect();
        Self { paths }
    }

    /// The property at JSON path `path`.
    pub fn at(&self, path: &[&str]) -> Option<&'static str> {
        self.paths
            .iter()
            .find(|(_, p)| p.len() == path.len() && p.iter().zip(path).all(|(a, b)| a == b))
            .map(|(name, _)| *name)
    }

    /// Whether top-level key `key` is a `#[sub_props]` group.
    pub fn is_group(&self, key: &str) -> bool {
        self.paths
            .iter()
            .any(|(_, path)| path.len() == 2 && path[0] == key)
    }

    /// The JSON path of property `name`.
    pub fn path_of(&self, name: &str) -> Option<&[String]> {
        self.paths
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, path)| path.as_slice())
    }
}

/// The path of property `name` (whose default serializes to `value`) in
/// `shape`: top level when the field there holds that value, otherwise the
/// group holding it, otherwise top level.
fn locate(shape: &Map<String, Value>, name: &str, value: Option<&Value>) -> Vec<String> {
    let matches = |found: &Value| match value {
        Some(value) => values_equal(found, value),
        None => !found.is_object(),
    };
    if shape.get(name).is_some_and(matches) {
        return vec![name.to_string()];
    }
    shape
        .iter()
        .find(|(_, group)| group.get(name).is_some_and(matches))
        .map(|(group, _)| vec![group.clone(), name.to_string()])
        .unwrap_or_else(|| vec![name.to_string()])
}

/// The cached [`PropertyLayout`] of `class`.
pub fn layout(class: &str) -> Arc<PropertyLayout> {
    static LAYOUTS: OnceLock<Mutex<HashMap<String, Arc<PropertyLayout>>>> = OnceLock::new();
    let mut layouts = LAYOUTS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    Arc::clone(
        layouts
            .entry(class.to_string())
            .or_insert_with(|| Arc::new(PropertyLayout::compute(class))),
    )
}

// ── Cache ──────────────────────────────────────────────────────────────────

/// The template of `def`, built once per class definition: a later call
/// with the same definition returns the cached template; a changed
/// definition (an edited prefab) replaces it.
pub fn template(def: &ClassDefinition) -> Arc<ClassTemplate> {
    type Cache = HashMap<(ClassId, std::path::PathBuf), (u64, Arc<ClassTemplate>)>;
    static TEMPLATES: OnceLock<Mutex<Cache>> = OnceLock::new();
    let fingerprint = fingerprint(def);
    let key = (def.id.clone(), def.dir.clone());
    let cache = TEMPLATES.get_or_init(Default::default);
    if let Some((cached, template)) = cache
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .get(&key)
    {
        if *cached == fingerprint {
            return Arc::clone(template);
        }
    }
    // Built outside the lock: decoding may load assets.
    let built = Arc::new(ClassTemplate::build(def.clone()));
    cache
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .insert(key, (fingerprint, Arc::clone(&built)));
    built
}

fn fingerprint(def: &ClassDefinition) -> u64 {
    let mut hasher = DefaultHasher::new();
    def.name.hash(&mut hasher);
    for component in &def.prefab.components {
        component.slot_id.hash(&mut hasher);
        component.class_name.hash(&mut hasher);
        component.enabled.hash(&mut hasher);
        component.data.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prefab::{PrefabAsset, PrefabComponent};
    use serde_json::json;

    fn def(components: Vec<PrefabComponent>) -> ClassDefinition {
        ClassDefinition {
            name: "Test".into(),
            prefab: PrefabAsset {
                components,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn an_unregistered_slot_keeps_its_patched_payload() {
        let def = def(vec![PrefabComponent {
            slot_id: "a".into(),
            class_name: "NotRegistered".into(),
            enabled: true,
            data: json!({"v": 1, "w": 2, "__slot_id": "a"}),
        }]);
        let template = ClassTemplate::build(def);
        match template.slot_value("a", Some(&json!({"w": 9}))) {
            Some(InstanceValue::Unresolved(unresolved)) => {
                assert_eq!(unresolved.data, json!({"v": 1, "w": 9}))
            }
            _ => panic!("expected an unresolved payload"),
        }
        assert!(template.slot_value("missing", None).is_none());
    }

    #[test]
    fn the_cache_follows_the_definition() {
        let component = |v: i64| PrefabComponent {
            slot_id: "a".into(),
            class_name: "NotRegistered".into(),
            enabled: true,
            data: json!({ "v": v }),
        };
        let first = template(&def(vec![component(1)]));
        assert!(Arc::ptr_eq(&first, &template(&def(vec![component(1)]))));
        let edited = template(&def(vec![component(2)]));
        assert!(!Arc::ptr_eq(&first, &edited));
        assert_eq!(edited.slot("a").unwrap().default_data, json!({"v": 2}));
    }
}
