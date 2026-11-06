//! Scene file helpers shared with `engine_backend`: component instance
//! records from a scene object's props, and transform composition. Legacy
//! record shapes are migrated with the file (`pulsar_class::records`).
//!
//! The legacy import adapter that dispatched records through
//! `ComponentRuntimeBehavior::sync_component` straight into a Helio scene
//! is gone (Pulsar-Native#1035, Phase 4): every scene is hydrated into
//! SceneDB, and components reach the renderer through their data.

use std::collections::HashMap;

use serde_json::Value;

// ── Shared public API (called by engine_backend) ─────────────────────────

/// Extract `(index, class_name, data)` from a component-instances value.
///
/// Prefers the explicit `component_instances` parameter (the modern path).
/// Falls back to `props["__component_instances"]` for backward compatibility
/// with v1/v2 scene files that embed this data inside the props map.
pub fn component_instances_from_props(
    props: &HashMap<String, Value>,
    component_instances: Option<&Value>,
) -> Vec<(usize, String, Value)> {
    let arr = component_instances.and_then(|v| v.as_array()).or_else(|| {
        props
            .get("__component_instances")
            .and_then(|v| v.as_array())
    });
    let Some(arr) = arr else {
        return Vec::new();
    };
    let records: Vec<_> = arr
        .iter()
        .enumerate()
        .filter_map(|(fi, entry)| {
            let o = entry.as_object()?;
            let idx = o
                .get("index")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize)
                .unwrap_or(fi);
            let cls = o
                .get("class_name")
                .and_then(|v| v.as_str())
                .map(str::to_string)?;
            let dat = o.get("data").cloned().unwrap_or(Value::Null);
            Some((idx, cls, dat))
        })
        .collect();
    records
}

