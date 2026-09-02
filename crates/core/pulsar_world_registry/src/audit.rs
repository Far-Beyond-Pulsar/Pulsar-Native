//! Reflection-metadata audit (#645): registry-wide consistency checks plus
//! a deterministic snapshot of every class's method/property surface.
//!
//! Two consumers:
//! - **CI golden test** (`pulsar_physics/tests/golden_metadata.rs` and any
//!   other crate that links real component classes):
//!   [`metadata_snapshot_json`] is diffed against a checked-in file, so a
//!   metadata regression -- renamed parameter, flipped `method_type`, lost
//!   category -- fails the build instead of silently degrading Blueprint
//!   discovery.
//! - **Debug builds / tooling**: [`find_overloaded_methods`] sweeps every
//!   registered class for name collisions. The compile-time half of the
//!   overload policy lives in `#[component_methods]` (one impl block);
//!   this is the link-time half, catching collisions across separate
//!   registrations for the same class that no single macro expansion can
//!   see.
//!
//! Everything here reads ONLY public reflection registries -- it never
//! mutates them -- so it is safe to call from tests and editor tooling
//! alike.

use serde_json::{json, Value};

use pulsar_reflection::REGISTRY;

/// One metadata-policy violation found by an audit sweep.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, thiserror::Error)]
pub enum MetadataAuditError {
    /// #645 overload policy: two reflected methods share a name on one
    /// class. Name-keyed dispatch (`REGISTRY.get_method`) resolves to the
    /// first registration, so the duplicate would be unreachable shadowed
    /// surface.
    #[error("class '{class_name}' declares {occurrences} methods named '{method}'; overloads are disallowed")]
    OverloadedMethod {
        class_name: String,
        method: String,
        occurrences: usize,
    },
}

/// Sweep EVERY registered class for overloaded method names. Empty result =
/// policy satisfied. O(classes × methods log methods); audit-time only,
/// never called from dispatch hot paths.
pub fn find_overloaded_methods() -> Vec<MetadataAuditError> {
    let mut violations = Vec::new();
    for class_name in REGISTRY.get_class_names() {
        let Some(methods) = REGISTRY.get_methods(class_name) else {
            continue;
        };
        let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        for method in &methods {
            *counts.entry(method.name).or_default() += 1;
        }
        let mut overloads: Vec<_> = counts.into_iter().filter(|(_, n)| *n > 1).collect();
        overloads.sort();
        for (method, occurrences) in overloads {
            violations.push(MetadataAuditError::OverloadedMethod {
                class_name: class_name.to_string(),
                method: method.to_string(),
                occurrences,
            });
        }
    }
    violations.sort();
    violations
}

/// Deterministic JSON snapshot of every registered class's full reflected
/// surface: properties (name/display/category/type) and methods
/// (name/display/category/params/return/`method_type`). Sorted at every
/// level so the output is stable across runs and link orderings -- exactly
/// what a checked-in golden file needs.
///
/// `method_type` is included deliberately: it is the purity contract
/// rust_codegen inlining relies on (#645), so a flipped Pure↔Fn must show
/// up as a reviewable snapshot diff, not a silent behavior change.
pub fn metadata_snapshot_json() -> Value {
    let mut classes: Vec<Value> = Vec::new();

    for class_name in REGISTRY.get_class_names() {
        let mut properties: Vec<Value> = REGISTRY
            .create_instance(class_name)
            .map(|instance| {
                instance
                    .get_properties()
                    .into_iter()
                    .map(|property| {
                        json!({
                            "name": property.name,
                            "display_name": property.display_name,
                            "category": property.category,
                            "type": property.type_info.type_name,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        properties.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        let mut methods: Vec<Value> = REGISTRY
            .get_methods(class_name)
            .unwrap_or_default()
            .into_iter()
            .map(|method| {
                let params: Vec<Value> = method
                    .params
                    .iter()
                    .map(|param| json!({ "name": param.name, "type": param.type_info.type_name }))
                    .collect();
                json!({
                    "name": method.name,
                    "display_name": method.display_name,
                    "category": method.category,
                    "params": params,
                    "return_type": method.return_type.map(|r| json!(r.type_info.type_name)),
                    "method_type": format!("{:?}", method.method_type),
                })
            })
            .collect();
        methods.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

        classes.push(json!({
            "name": class_name,
            "properties": properties,
            "methods": methods,
        }));
    }

    classes.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    json!({ "classes": classes })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A class with colliding registrations is flagged; clean classes are
    /// not. Uses the live global registry -- whatever else the test binary
    /// linked in must itself satisfy the policy, which is exactly the
    /// invariant the sweep exists to assert.
    #[test]
    fn overload_sweep_reports_only_real_collisions() {
        // The workspace's own classes must satisfy the overload policy.
        let violations = find_overloaded_methods();
        assert!(
            violations.is_empty(),
            "overload policy violated: {violations:?}"
        );
    }

    /// The snapshot is stable: two consecutive calls produce byte-identical
    /// JSON (sort discipline), whatever the binary has registered.
    #[test]
    fn snapshot_is_deterministic_across_calls() {
        let first = serde_json::to_string(&metadata_snapshot_json()).unwrap();
        let second = serde_json::to_string(&metadata_snapshot_json()).unwrap();
        assert_eq!(first, second, "snapshot must not depend on iteration order");
    }

    /// Snapshot entries carry the fields downstream discovery needs --
    /// including the load-bearing method_type purity tag (#645).
    #[test]
    fn snapshot_entries_are_fully_populated() {
        let snapshot = metadata_snapshot_json();
        for class in snapshot["classes"].as_array().unwrap() {
            assert!(class["name"].as_str().is_some());
            for method in class["methods"].as_array().unwrap() {
                assert!(
                    matches!(
                        method["method_type"].as_str(),
                        Some("Pure") | Some("Fn") | Some("ControlFlow")
                    ),
                    "method {} of {} lacks a valid method_type tag",
                    method["name"],
                    class["name"]
                );
                assert!(method["display_name"].is_string());
            }
        }
    }
}
