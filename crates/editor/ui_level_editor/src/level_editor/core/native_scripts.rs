//! Rust-script actor data model (#653): the editor-side record that binds a
//! scene object to a gameplay script crate's actor type.
//!
//! Data-model level only: a Rust actor object carries a
//! `NativeScriptComponent` (`pulsar_class::NativeScriptComponent`, #921):
//!
//! ```json
//! { "class_name": "NativeScriptComponent",
//!   "enabled": true,
//!   "data": { "script_crate": "game_scripts", "actor_type": "Spinner" } }
//! ```
//!
//! Blueprint classes are placed as `ClassInstance`s instead; native script
//! crates are not class assets (no class directory or GUID), so they keep
//! this small component. Levels from before #921 stored the same binding as
//! the "rust mode" of the retired `ScriptComponent`; the level migration
//! (`pulsar_class::migrate`) converts it.
//!
//! The record rides the existing save path (`component_instances` round-trips;
//! D5 proved re-saves preserve it). What consumes it today is discovery +
//! authoring; binding-driven SPAWNING at play time (a level-format section
//! like #650's `blueprint_bindings`) and inspector UI are F's follow-up — see
//! the E3 handoff.

use serde_json::{json, Value};

use crate::level_editor::scene_edit::{ObjectType, SceneObjectData, Transform};

/// Class name of the component that binds an object to a Rust actor.
///
/// F's inspector/menu work consumes these helpers (E3 landed the data model
/// only); until then they are deliberately allowed dead.
#[allow(dead_code)]
pub const NATIVE_SCRIPT_CLASS: &str = pulsar_class::native_script::NATIVE_SCRIPT_COMPONENT;

/// Build the `component_instances` array entry binding an object to a Rust
/// actor from a script crate.
#[allow(dead_code)]
pub fn rust_script_instance(crate_name: &str, actor_type: &str) -> Value {
    json!({
        "class_name": NATIVE_SCRIPT_CLASS,
        "enabled": true,
        "data": serde_json::to_value(pulsar_class::NativeScriptComponent::new(crate_name, actor_type))
            .unwrap_or_default(),
    })
}

/// Read back a Rust-script binding from an object's `component_instances`.
///
/// Returns `(script_crate, actor_type)` for the FIRST
/// `NativeScriptComponent` entry; other components yield `None`. Tolerates
/// missing/foreign shapes — discovery data must never make scene loading
/// fragile.
#[allow(dead_code)]
pub fn find_rust_script_binding(component_instances: Option<&Value>) -> Option<(String, String)> {
    let arr = component_instances?.as_array()?;
    arr.iter()
        .find(|inst| inst.get("class_name").and_then(|v| v.as_str()) == Some(NATIVE_SCRIPT_CLASS))
        .and_then(|inst| {
            let data = inst.get("data")?;
            let crate_name = data.get("script_crate")?.as_str()?.to_string();
            let actor_type = data.get("actor_type")?.as_str()?.to_string();
            Some((crate_name, actor_type))
        })
}

/// Build a new scene object pre-bound to a Rust script actor: named after the
/// type, carrying its `NativeScriptComponent`. Consumed by the
/// add-object flow (F wires the menu; this owns the DATA so both the menu and
/// tests agree on one shape).
#[allow(dead_code)]
pub fn rust_script_object_data(crate_name: &str, actor_type: &str) -> SceneObjectData {
    SceneObjectData {
        id: String::new(),
        name: actor_type.to_string(),
        object_type: ObjectType::Empty,
        transform: Transform::default(),
        visible: true,
        locked: false,
        parent: None,
        children: vec![],
        scene_path: String::new(),
        props: Default::default(),
        component_instances: Some(Value::Array(vec![rust_script_instance(
            crate_name, actor_type,
        )])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round trip: build → read back yields the same (crate, type) pair.
    #[test]
    fn rust_binding_round_trips_through_the_instance_record() {
        let obj = rust_script_object_data("game_scripts", "Spinner");
        let instances = obj.component_instances.as_ref().expect("instances set");

        assert_eq!(
            find_rust_script_binding(Some(instances)),
            Some(("game_scripts".to_string(), "Spinner".to_string())),
        );
    }

    /// Class instances and other components never read as rust bindings.
    #[test]
    fn class_instances_and_foreign_entries_do_not_read_as_rust() {
        let class_only = json!([
            { "class_name": "ClassInstance", "enabled": true,
              "data": { "class": "6f1c1a52-1d7e-4d7e-9c55-2b7c1f0d7a10" } },
        ]);
        assert_eq!(find_rust_script_binding(Some(&class_only)), None);

        assert_eq!(find_rust_script_binding(None), None);
        assert_eq!(find_rust_script_binding(Some(&json!("garbage"))), None);

        // Mixed arrays pick only the rust-mode entry.
        let mixed = json!([
            { "class_name": "LightComponent", "enabled": true, "data": {} },
            { "class_name": "NativeScriptComponent", "enabled": true,
              "data": { "script_crate": "c", "actor_type": "T" } },
        ]);
        assert_eq!(
            find_rust_script_binding(Some(&mixed)),
            Some(("c".to_string(), "T".to_string())),
        );
    }

    /// A missing crate/type field makes the record unreadable rather than
    /// half-matching — malformed bindings surface as `None`, never a panic.
    #[test]
    fn malformed_rust_records_are_typed_as_none() {
        let broken = json!([
            { "class_name": "NativeScriptComponent", "data": { "script_crate": "c" } },
        ]);
        assert_eq!(find_rust_script_binding(Some(&broken)), None);
    }

    /// The produced object is add-object-flow ready: fresh id, type-named,
    /// default transform — same shape `on_add_object_of_type` constructs.
    #[test]
    fn rust_script_object_is_a_valid_add_command_payload() {
        let obj = rust_script_object_data("extra_scripts", "Bouncer");
        assert_eq!(obj.name, "Bouncer");
        assert!(obj.id.is_empty(), "command assigns the id");
        assert_eq!(obj.transform.position, [0.0; 3]);
        assert_eq!(obj.transform.scale, [1.0; 3]);
        assert_eq!(
            find_rust_script_binding(obj.component_instances.as_ref()),
            Some(("extra_scripts".to_string(), "Bouncer".to_string())),
        );
    }
}
