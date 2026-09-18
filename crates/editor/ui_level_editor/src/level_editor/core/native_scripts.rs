//! Rust-script actor data model (#653): the editor-side record that binds a
//! scene object to a gameplay script crate's actor type.
//!
//! Data-model level only, mirroring how Blueprint objects carry a
//! `ScriptComponent` with `script_asset` (see
//! `scene_database::find_script_path`): a Rust actor object carries a
//! `ScriptComponent` instance whose `data` uses the documented RUST mode:
//!
//! ```json
//! { "class_name": "ScriptComponent",
//!   "enabled": true,
//!   "data": { "mode": "rust", "script_crate": "game_scripts",
//!             "actor_type": "Spinner" } }
//! ```
//!
//! The record rides the existing save path (`component_instances` round-trips;
//! D5 proved re-saves preserve it). What consumes it today is discovery +
//! authoring; binding-driven SPAWNING at play time (a level-format section
//! like #650's `blueprint_bindings`) and inspector UI are F's follow-up — see
//! the E3 handoff.

use serde_json::{json, Value};

use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};

/// Class name of the component instance that carries script bindings.
///
/// F's inspector/menu work consumes these helpers (E3 landed the data model
/// only); until then they are deliberately allowed dead.
#[allow(dead_code)]
pub const SCRIPT_COMPONENT_CLASS: &str = "ScriptComponent";

/// Build the `component_instances` array entry binding an object to a Rust
/// actor from a script crate (the RUST mode of `ScriptComponent`).
#[allow(dead_code)]
pub fn rust_script_instance(crate_name: &str, actor_type: &str) -> Value {
    json!({
        "class_name": SCRIPT_COMPONENT_CLASS,
        "enabled": true,
        "data": {
            "mode": "rust",
            "script_crate": crate_name,
            "actor_type": actor_type,
        },
    })
}

/// Read back a Rust-script binding from an object's `component_instances`.
///
/// Returns `(script_crate, actor_type)` for the FIRST rust-mode
/// ScriptComponent entry; blueprint-mode entries (`script_asset`) and other
/// components yield `None`. Tolerates missing/foreign shapes — discovery data
/// must never make scene loading fragile.
#[allow(dead_code)]
pub fn find_rust_script_binding(component_instances: Option<&Value>) -> Option<(String, String)> {
    let arr = component_instances?.as_array()?;
    arr.iter()
        .find(|inst| {
            inst.get("class_name").and_then(|v| v.as_str()) == Some(SCRIPT_COMPONENT_CLASS)
                && inst
                    .get("data")
                    .and_then(|d| d.get("mode"))
                    .and_then(|m| m.as_str())
                    == Some("rust")
        })
        .and_then(|inst| {
            let data = inst.get("data")?;
            let crate_name = data.get("script_crate")?.as_str()?.to_string();
            let actor_type = data.get("actor_type")?.as_str()?.to_string();
            Some((crate_name, actor_type))
        })
}

/// Build a new scene object pre-bound to a Rust script actor: named after the
/// type, carrying the rust-mode `ScriptComponent` instance. Consumed by the
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

    /// Blueprint-mode ScriptComponents and non-Script entries never read as
    /// rust bindings — the two modes stay distinguishable at the data layer.
    #[test]
    fn blueprint_mode_and_foreign_entries_do_not_read_as_rust() {
        let blueprint_only = json!([
            { "class_name": "ScriptComponent", "enabled": true,
              "data": { "script_asset": "src/classes/Foo/graph_save.json" } },
        ]);
        assert_eq!(find_rust_script_binding(Some(&blueprint_only)), None);

        assert_eq!(find_rust_script_binding(None), None);
        assert_eq!(find_rust_script_binding(Some(&json!("garbage"))), None);

        // Mixed arrays pick only the rust-mode entry.
        let mixed = json!([
            { "class_name": "LightComponent", "enabled": true, "data": {} },
            { "class_name": "ScriptComponent", "enabled": true,
              "data": { "mode": "rust", "script_crate": "c", "actor_type": "T" } },
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
            { "class_name": "ScriptComponent", "data": { "mode": "rust", "script_crate": "c" } },
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
