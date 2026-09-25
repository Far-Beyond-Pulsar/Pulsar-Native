//! JSON diffs for per-instance overrides.
//!
//! A component override is the part of an instance's component data that
//! differs from its class slot's default: objects are diffed key by key,
//! recursively, so a class edit to any value the instance did not override
//! still reaches it. `__`-prefixed keys are metadata and never diffed.

use std::collections::{BTreeMap, HashMap};

use pulsar_reflection::{REGISTRY, RUNTIME_TYPE_REGISTRY};
use serde_json::{Map, Value};

/// `true` for editor/class metadata keys (`__slot_id`, `__parent_index`, …).
pub fn is_meta_key(key: &str) -> bool {
    key.starts_with("__")
}

/// JSON equality that tolerates float noise (an `f32` written as `0.1`
/// reads back as `0.10000000149011612`).
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => {
                let scale = x.abs().max(y.abs()).max(1.0);
                (x - y).abs() <= 1e-6 * scale
            }
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(x, y)| values_equal(x, y))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| values_equal(v, w)))
        }
        _ => a == b,
    }
}

/// The part of `current` that differs from `default`, or `None` when they
/// match. Keys only in `default` are ignored (the instance keeps the class
/// value); metadata keys are skipped.
pub fn diff(default: &Value, current: &Value) -> Option<Value> {
    match (default, current) {
        (Value::Object(d), Value::Object(c)) => {
            let mut out = Map::new();
            for (key, value) in c {
                if is_meta_key(key) {
                    continue;
                }
                match d.get(key) {
                    Some(dv) => {
                        if let Some(sub) = diff(dv, value) {
                            out.insert(key.clone(), sub);
                        }
                    }
                    None => {
                        out.insert(key.clone(), value.clone());
                    }
                }
            }
            (!out.is_empty()).then_some(Value::Object(out))
        }
        _ => (!values_equal(default, current)).then(|| current.clone()),
    }
}

/// Apply a diff produced by [`diff`] onto `base`: objects merge key by key,
/// anything else replaces. Metadata keys in `patch` are ignored.
pub fn apply(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(b), Value::Object(p)) => {
            for (key, value) in p {
                if is_meta_key(key) {
                    continue;
                }
                match b.get_mut(key) {
                    Some(slot) if slot.is_object() && value.is_object() => apply(slot, value),
                    _ => {
                        b.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (base, patch) => *base = patch.clone(),
    }
}

/// Split `data` into its metadata keys and the rest.
pub fn split_meta(data: &Value) -> (Map<String, Value>, Value) {
    let mut meta = Map::new();
    let rest = match data {
        Value::Object(map) => {
            let mut rest = Map::new();
            for (k, v) in map {
                if is_meta_key(k) {
                    meta.insert(k.clone(), v.clone());
                } else {
                    rest.insert(k.clone(), v.clone());
                }
            }
            Value::Object(rest)
        }
        other => other.clone(),
    };
    (meta, rest)
}

/// Bring a component's JSON into the shape its typed component serializes
/// to, so defaults and live values compare like with like.
///
/// Prefab data is written as a flat `property → value` map (one key per
/// reflected property), while components with `#[sub_props]` serialize
/// nested. This starts from the class's `Default`, applies every reflected
/// property found flat in `data`, serializes, then merges any keys `data`
/// already has in the nested shape. Unregistered classes, and classes that
/// cannot serialize, are returned unchanged. Metadata keys are kept.
pub fn normalize_component_data(class_name: &str, data: &Value) -> Value {
    let Some(mut instance) = REGISTRY.create_instance(class_name) else {
        return data.clone();
    };
    let (meta, body) = split_meta(data);
    if let Some(obj) = body.as_object() {
        for prop in instance.get_properties() {
            if let Some(raw) = obj.get(prop.name) {
                if let Ok(value) =
                    RUNTIME_TYPE_REGISTRY.deserialize_json_for_type(prop.type_info, raw.clone())
                {
                    (prop.setter)(instance.as_mut(), value);
                }
            }
        }
    }
    let Ok(mut full) = instance.to_json() else {
        return data.clone();
    };
    if let (Some(full_map), Some(body_map)) = (full.as_object_mut(), body.as_object()) {
        for (key, value) in body_map {
            if let Some(slot) = full_map.get_mut(key) {
                if slot.is_object() && value.is_object() {
                    merge_known_keys(slot, value);
                } else if same_kind(slot, value) {
                    *slot = value.clone();
                }
            }
        }
    }
    if let Some(full_map) = full.as_object_mut() {
        full_map.extend(meta);
    }
    full
}

/// Merge `patch` into `base`, only where `base` already has the key with a
/// compatible JSON kind (so a stray flat value never breaks the typed shape).
fn merge_known_keys(base: &mut Value, patch: &Value) {
    let (Some(b), Some(p)) = (base.as_object_mut(), patch.as_object()) else {
        return;
    };
    for (key, value) in p {
        if let Some(slot) = b.get_mut(key) {
            if slot.is_object() && value.is_object() {
                merge_known_keys(slot, value);
            } else if same_kind(slot, value) {
                *slot = value.clone();
            }
        }
    }
}

fn same_kind(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Whether a variable value equals the class default. Blueprint variable
/// defaults are stored as strings (`"5.0"`), overrides in their natural JSON
/// form (`5.0`), so a string default also matches the value it parses to.
pub fn variable_equals_default(default: &Value, value: &Value) -> bool {
    if values_equal(default, value) {
        return true;
    }
    match (default, value) {
        (Value::String(d), v) if !v.is_string() => serde_json::from_str::<Value>(d)
            .map(|parsed| values_equal(&parsed, v))
            .unwrap_or(false),
        _ => false,
    }
}

/// Drop variable overrides equal to the class default.
pub fn prune_variable_overrides(
    defaults: &HashMap<String, Value>,
    overrides: &BTreeMap<String, Value>,
) -> BTreeMap<String, Value> {
    overrides
        .iter()
        .filter(|(name, value)| {
            !defaults
                .get(name.as_str())
                .is_some_and(|default| variable_equals_default(default, value))
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Remove the property at `path` (dot-separated for nested objects) from an
/// override diff, dropping emptied objects. Returns whether it was present.
pub fn remove_path(diff: &mut Value, path: &str) -> bool {
    fn go(value: &mut Value, parts: &[&str]) -> bool {
        let Some(map) = value.as_object_mut() else {
            return false;
        };
        match parts {
            [] => false,
            [last] => map.remove(*last).is_some(),
            [head, rest @ ..] => {
                let Some(child) = map.get_mut(*head) else {
                    return false;
                };
                let removed = go(child, rest);
                if child.as_object().is_some_and(Map::is_empty) {
                    map.remove(*head);
                }
                removed
            }
        }
    }
    let parts: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
    go(diff, &parts)
}

/// Every leaf of a diff as `(dot.path, value)`, for display.
pub fn leaf_paths(diff: &Value) -> Vec<(String, Value)> {
    fn go(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
        match value.as_object() {
            Some(map) if !map.is_empty() => {
                for (k, v) in map {
                    if is_meta_key(k) {
                        continue;
                    }
                    let path = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    go(&path, v, out);
                }
            }
            _ => out.push((prefix.to_string(), value.clone())),
        }
    }
    let mut out = Vec::new();
    if diff.is_object() {
        go("", diff, &mut out);
    }
    out
}

/// Look up a dot-separated path in a JSON object.
pub fn get_path<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .filter(|p| !p.is_empty())
        .try_fold(value, |v, part| v.get(part))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn diff_is_recursive_and_minimal() {
        let default =
            json!({"a": 1, "nested": {"x": 1.0, "y": 2.0}, "s": "keep", "__slot_id": "A"});
        let current =
            json!({"a": 1, "nested": {"x": 1.0, "y": 5.0}, "s": "keep", "__slot_id": "A"});
        assert_eq!(
            diff(&default, &current),
            Some(json!({"nested": {"y": 5.0}}))
        );
        assert_eq!(diff(&default, &default), None);
    }

    #[test]
    fn float_noise_is_not_an_override() {
        let default = json!({"v": 0.1});
        let current = json!({"v": 0.1f32 as f64});
        assert_eq!(diff(&default, &current), None);
    }

    #[test]
    fn apply_round_trips_diff() {
        let default = json!({"a": 1, "nested": {"x": 1, "y": 2}});
        let current = json!({"a": 3, "nested": {"x": 1, "y": 7}});
        let d = diff(&default, &current).unwrap();
        let mut rebuilt = default.clone();
        apply(&mut rebuilt, &d);
        assert_eq!(rebuilt, current);
    }

    #[test]
    fn string_variable_defaults_match_parsed_values() {
        assert!(variable_equals_default(&json!("5.0"), &json!(5.0)));
        assert!(variable_equals_default(&json!("hi"), &json!("hi")));
        assert!(!variable_equals_default(&json!("5.0"), &json!(6.0)));
    }

    #[test]
    fn remove_path_prunes_empty_parents() {
        let mut d = json!({"nested": {"y": 5}, "a": 1});
        assert!(remove_path(&mut d, "nested.y"));
        assert_eq!(d, json!({"a": 1}));
        assert_eq!(leaf_paths(&json!({"n": {"x": 1}, "b": 2})).len(), 2);
    }
}
