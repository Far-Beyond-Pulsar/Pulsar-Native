//! The scene tools against an in-memory SceneDB world: each tool, validation
//! errors, transactions and undo, and sandboxes (diff, apply, discard).

mod common;

use common::{rig, strict_rig, Lamp};
use serde_json::json;

// ── Reads ────────────────────────────────────────────────────────────────────

#[test]
fn spawn_query_and_get_round_trip() {
    let r = rig();
    let spawned = r.ok(
        "scene_write_spawn_object",
        json!({
            "name": "Lamp A", "id": "lamp_a", "kind": "light_point", "position": [1, 2, 3],
            "components": [{"class_name": "Lamp", "properties": {"intensity": 5.0, "label": "warm"}}]
        }),
        "run-1",
    );
    assert_eq!(spawned["id"], "lamp_a");
    assert_eq!(spawned["transaction"]["author"], "agent:run-1");
    assert_eq!(spawned["transaction"]["undoable"], true);

    r.ok("scene_write_spawn_object", json!({"name": "Floor", "kind": "mesh_plane"}), "run-1");

    let all = r.ok("scene_query", json!({}), "run-1");
    assert_eq!(all["total_matches"], 2);
    assert_eq!(all["scene_object_count"], 2);

    let by_component = r.ok("scene_query", json!({"has_component": "Lamp"}), "run-1");
    assert_eq!(by_component["total_matches"], 1);
    assert_eq!(by_component["objects"][0]["id"], "lamp_a");
    assert_eq!(by_component["objects"][0]["position"], json!([1.0, 2.0, 3.0]));
    assert_eq!(by_component["objects"][0]["components"], json!(["Lamp"]));

    assert_eq!(r.ok("scene_query", json!({"name_contains": "lamp a"}), "r")["total_matches"], 1);
    assert_eq!(r.ok("scene_query", json!({"kind": "mesh_plane"}), "r")["total_matches"], 1);
    assert_eq!(r.ok("scene_query", json!({"kind": "mesh_cube"}), "r")["total_matches"], 0);

    let got = r.ok("scene_get_object", json!({"id": "lamp_a"}), "run-1");
    assert_eq!(got["kind"], "light_point");
    assert_eq!(got["components"][0]["class_name"], "Lamp");
    assert_eq!(got["components"][0]["data"]["intensity"], 5.0);
    assert_eq!(got["components"][0]["data"]["label"], "warm");
    let no_data = r.ok("scene_get_object", json!({"id": "lamp_a", "include_component_data": false}), "r");
    assert!(no_data["components"][0].get("data").is_none());
}

#[test]
fn query_pages_and_filters_hierarchy() {
    let r = rig();
    r.seed();
    for n in 0..5 {
        r.ok(
            "scene_write_spawn_object",
            json!({"name": format!("Extra {n}"), "id": format!("extra_{n}")}),
            "run-1",
        );
    }
    let page = r.ok("scene_query", json!({"limit": 3}), "run-1");
    assert_eq!(page["returned"], 3);
    assert_eq!(page["total_matches"], 7);
    assert_eq!(page["next_offset"], 3);
    let rest = r.ok("scene_query", json!({"limit": 10, "offset": 3}), "run-1");
    assert_eq!(rest["returned"], 4);
    assert!(rest.get("next_offset").is_none());

    assert_eq!(r.ok("scene_query", json!({"root_only": true}), "r")["total_matches"], 6);
    assert_eq!(r.ok("scene_query", json!({"descendants_of": "root"}), "r")["total_matches"], 1);
    assert_eq!(r.ok("scene_query", json!({"parent_id": "root"}), "r")["objects"][0]["id"], "child");
    // Hierarchy order: a parent comes before its children.
    let ordered = r.ok("scene_query", json!({"ids": ["child", "root"]}), "r");
    assert_eq!(ordered["objects"][0]["id"], "root");
}

#[test]
fn component_classes_and_description() {
    let r = rig();
    let classes = r.ok("scene_list_component_classes", json!({"name_contains": "lam"}), "r");
    assert!(classes["classes"].as_array().unwrap().iter().any(|c| c == "Lamp"));
    let described = r.ok("scene_describe_component", json!({"class_name": "Lamp"}), "r");
    let paths: Vec<&str> = described["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"intensity") && paths.contains(&"label") && paths.contains(&"on"), "{paths:?}");
    assert!(described["example_properties"].as_object().is_some_and(|m| !m.is_empty()));
}

// ── Validation ───────────────────────────────────────────────────────────────

#[test]
fn errors_are_model_readable() {
    let r = rig();
    r.seed();
    let before = r.image();

    let unknown = r.err(
        "scene_write_add_component",
        json!({"id": "root", "class_name": "Lampp"}),
        "run-1",
    );
    assert!(unknown.contains("Unknown component class 'Lampp'"), "{unknown}");
    assert!(unknown.contains("Lamp"), "should suggest the real class: {unknown}");
    assert!(unknown.contains("scene_list_component_classes"), "{unknown}");

    let wrong_type = r.err(
        "scene_write_set_component",
        json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": "bright"}}),
        "run-1",
    );
    assert!(wrong_type.contains("Data does not fit Lamp"), "{wrong_type}");
    assert!(wrong_type.contains("intensity"), "{wrong_type}");

    let bad_field = r.err(
        "scene_write_set_component",
        json!({"id": "root", "class_name": "Lamp", "properties": {"intesity": 1.0}}),
        "run-1",
    );
    assert!(bad_field.contains("there is no field `intesity`"), "{bad_field}");
    assert!(bad_field.contains("intensity (number)"), "lists the real fields: {bad_field}");

    let missing = r.err("scene_write_set_object", json!({"id": "chil", "name": "x"}), "run-1");
    assert!(missing.contains("No object with id 'chil'"), "{missing}");
    assert!(missing.contains("Similar ids"), "{missing}");

    let dup = r.err("scene_write_spawn_object", json!({"name": "x", "id": "root"}), "run-1");
    assert!(dup.contains("already exists"), "{dup}");

    let kind = r.err("scene_write_spawn_object", json!({"name": "x", "kind": "mesh_cubee"}), "run-1");
    assert!(kind.contains("Valid kinds"), "{kind}");

    let vec = r.err("scene_write_transform", json!({"id": "root", "position": [1, 2]}), "run-1");
    assert!(vec.contains("three numbers"), "{vec}");

    let both = r.err(
        "scene_write_transform",
        json!({"id": "root", "position": [1, 2, 3], "translate": [1, 0, 0]}),
        "run-1",
    );
    assert!(both.contains("not both"), "{both}");

    let cycle = r.err("scene_write_set_object", json!({"id": "root", "parent_id": "child"}), "run-1");
    assert!(cycle.contains("its own ancestor"), "{cycle}");

    let args = r.err("scene_query", json!({"nmae_contains": "x"}), "run-1");
    assert!(args.contains("Invalid arguments for scene_query"), "{args}");

    let nothing = r.err("scene_write_set_component", json!({"id": "root", "class_name": "Lamp"}), "run-1");
    assert!(nothing.contains("Nothing to change"), "{nothing}");

    // Nothing was written and nothing was journaled.
    r.assert_unchanged_from(&before);
    assert!(r.tb.log().list_tree("run-1").is_empty());
}

#[test]
fn a_failed_call_rolls_back_what_it_already_wrote() {
    let r = strict_rig();
    let before = r.image();
    // The first component is fine, the second is refused by a level rule
    // after the object and the first component exist.
    let err = r.err(
        "scene_write_spawn_object",
        json!({"name": "Bad", "id": "bad", "components": [
            {"class_name": "Lamp", "properties": {"label": "ok"}},
            {"class_name": "Lamp", "properties": {"label": "forbidden"}}
        ]}),
        "run-1",
    );
    assert!(err.contains("a level cannot have a forbidden lamp"), "{err}");
    r.assert_unchanged_from(&before);
    assert!(r.tb.log().list_tree("run-1").is_empty());

    // An edit refused after the write is rolled back too.
    r.ok("scene_write_spawn_object", json!({"name": "A", "id": "a", "components": [{"class_name": "Lamp"}]}), "run-1");
    let ok_state = r.image();
    let err = r.err(
        "scene_write_set_component",
        json!({"id": "a", "class_name": "Lamp", "properties": {"label": "forbidden", "intensity": 9.0}}),
        "run-1",
    );
    assert!(err.contains("refused"), "{err}");
    r.assert_unchanged_from(&ok_state);
    assert_eq!(r.tb.log().list_tree("run-1").len(), 1, "only the spawn was journaled");
}

// ── Edits ────────────────────────────────────────────────────────────────────

#[test]
fn set_component_changes_only_named_fields() {
    let r = rig();
    r.seed();
    r.ok(
        "scene_write_set_component",
        json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 7.5}, "enabled": false}),
        "run-1",
    );
    let got = r.ok("scene_get_object", json!({"id": "root"}), "r");
    let lamp = &got["components"][0];
    assert_eq!(lamp["data"]["intensity"], 7.5);
    assert_eq!(lamp["data"]["label"], "seed", "unnamed fields keep their value");
    assert_eq!(lamp["enabled"], false);

    // By index, and the typed world value really changed.
    r.ok(
        "scene_write_set_component",
        json!({"id": "child", "component_index": 0, "properties": {"on": true}}),
        "run-1",
    );
    let db = r.scene.read();
    let on = db
        .world
        .query::<&Lamp>()
        .filter(|(_, l)| l.on)
        .count();
    assert_eq!(on, 1);
}

#[test]
fn add_and_remove_component() {
    let r = rig();
    r.seed();
    let added = r.ok(
        "scene_write_add_component",
        json!({"id": "child", "class_name": "Lamp", "properties": {"label": "second"}, "enabled": false}),
        "run-1",
    );
    assert_eq!(added["component_index"], 1);
    let got = r.ok("scene_get_object", json!({"id": "child"}), "r");
    assert_eq!(got["components"].as_array().unwrap().len(), 2);
    assert_eq!(got["components"][1]["enabled"], false);

    let removed = r.ok(
        "scene_write_remove_component",
        json!({"id": "child", "component_index": 1}),
        "run-1",
    );
    assert_eq!(removed["removed"], "Lamp");
    assert_eq!(r.ok("scene_get_object", json!({"id": "child"}), "r")["components"].as_array().unwrap().len(), 1);
    let err = r.err("scene_write_remove_component", json!({"id": "child", "component_index": 5}), "run-1");
    assert!(err.contains("out of range"), "{err}");
}

#[test]
fn transform_absolute_relative_and_children() {
    let r = rig();
    r.seed();
    r.ok("scene_write_transform", json!({"id": "root", "position": [10, 0, 0], "rotation": [0, 90, 0]}), "run-1");
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["position"], json!([10.0, 0.0, 0.0]));
    assert_eq!(root["rotation"], json!([0.0, 90.0, 0.0]));
    // Flat world-space transforms: the child did not move.
    assert_eq!(r.ok("scene_get_object", json!({"id": "child"}), "r")["position"], json!([1.0, 0.0, 0.0]));

    let moved = r.ok(
        "scene_write_transform",
        json!({"ids": ["root"], "translate": [0, 5, 0], "scale_by": [2, 2, 2], "include_children": true}),
        "run-1",
    );
    assert_eq!(moved["changed"], json!(["root", "child"]));
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["position"], json!([10.0, 5.0, 0.0]));
    assert_eq!(root["scale"], json!([2.0, 2.0, 2.0]));
    assert_eq!(r.ok("scene_get_object", json!({"id": "child"}), "r")["position"], json!([1.0, 5.0, 0.0]));
}

#[test]
fn set_object_renames_hides_and_reparents() {
    let r = rig();
    r.seed();
    r.ok("scene_write_spawn_object", json!({"name": "Other", "id": "other"}), "run-1");
    r.ok(
        "scene_write_set_object",
        json!({"id": "child", "name": "Kid", "visible": false, "locked": true, "parent_id": "other"}),
        "run-1",
    );
    let child = r.ok("scene_get_object", json!({"id": "child"}), "r");
    assert_eq!(child["name"], "Kid");
    assert_eq!(child["visible"], false);
    assert_eq!(child["locked"], true);
    assert_eq!(child["parent_id"], "other");
    r.ok("scene_write_set_object", json!({"id": "child", "parent_id": null}), "run-1");
    assert_eq!(r.ok("scene_get_object", json!({"id": "child"}), "r")["parent_id"], json!(null));
}

#[test]
fn delete_removes_the_subtree() {
    let r = rig();
    r.seed();
    let deleted = r.ok("scene_write_delete_object", json!({"id": "root"}), "run-1");
    assert_eq!(deleted["objects_removed"], 2);
    assert!(r.ids().is_empty());
    assert_eq!(r.scene.read().world.query::<&Lamp>().count(), 0, "component instances go with their object");
}

// ── Transactions and undo ────────────────────────────────────────────────────

#[test]
fn each_call_is_one_attributed_transaction_and_a_run_reverts_exactly() {
    let r = rig();
    r.seed();
    let baseline = r.image();

    r.ok("scene_write_spawn_object", json!({"name": "New", "id": "new", "parent_id": "root",
        "components": [{"class_name": "Lamp", "properties": {"label": "n"}}]}), "run-1");
    r.ok("scene_write_transform", json!({"id": "child", "translate": [0, 0, 9]}), "run-1");
    r.ok("scene_write_set_component", json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 99.0}}), "run-1");
    r.ok("scene_write_set_object", json!({"id": "child", "parent_id": "new", "name": "Moved"}), "run-1");
    r.ok("scene_write_delete_object", json!({"id": "root"}), "run-1");
    assert_eq!(r.ids().len(), 0);

    let log = r.tb.log().list_tree("run-1");
    assert_eq!(log.len(), 5, "one transaction per call");
    assert!(log.iter().all(|(_, author, _, _)| author == "agent:run-1"));
    assert_eq!(log[4].2, "scene_write_delete_object");

    let outcome = {
        let mut db = r.scene.write();
        r.tb.log().revert_tree("run-1", &mut db.world, scene_key(&r), false)
    };
    assert_eq!(outcome.reverted, 5, "{outcome:?}");
    assert!(outcome.failed.is_empty());
    r.assert_unchanged_from(&baseline);
    assert!(r.tb.log().list_tree("run-1").is_empty());
    // Reverting twice does nothing more.
    let again = {
        let mut db = r.scene.write();
        r.tb.log().revert_tree("run-1", &mut db.world, scene_key(&r), false)
    };
    assert_eq!(again.reverted, 0);
}

fn scene_key(r: &common::Rig) -> usize {
    std::sync::Arc::as_ptr(&r.scene) as *const u8 as usize
}

#[test]
fn revert_is_per_run_tree() {
    let r = rig();
    r.seed();
    r.ok("scene_write_spawn_object", json!({"name": "A", "id": "a"}), "run-1");
    r.ok("scene_write_spawn_object", json!({"name": "B", "id": "b"}), "run-1.c1");
    r.ok("scene_write_spawn_object", json!({"name": "C", "id": "c"}), "run-10");
    r.ok("scene_write_spawn_object", json!({"name": "D", "id": "d"}), "run-2");

    assert!(r.tb.log().has_undo("run-1"));
    let notes = r.tb.revert_tree("run-1").unwrap();
    assert!(notes.iter().any(|n| n.contains("2 scene transaction")), "{notes:?}");
    let ids = r.ids();
    assert!(!ids.contains(&"a".to_string()) && !ids.contains(&"b".to_string()));
    // `run-10` is not below `run-1`.
    assert!(ids.contains(&"c".to_string()) && ids.contains(&"d".to_string()), "{ids:?}");
    assert!(!r.tb.log().has_undo("run-1"));
    assert!(r.tb.log().has_undo("run-2"));
}

#[test]
fn revert_refuses_a_scene_that_is_no_longer_open() {
    let r = rig();
    r.ok("scene_write_spawn_object", json!({"name": "A", "id": "a"}), "run-1");
    let mut other = pulsar_scenedb::SceneDb::new();
    let outcome = r.tb.log().revert_tree("run-1", &mut other.world, 12345, false);
    assert_eq!(outcome.reverted, 0);
    assert_eq!(outcome.failed.len(), 1);
    assert!(outcome.failed[0].contains("no longer open"));
    assert_eq!(r.ids(), vec!["a".to_string()], "the live scene was not touched");
}

// ── Sandboxes ────────────────────────────────────────────────────────────────

fn sandbox_session(r: &common::Rig) -> &'static str {
    let key = "run-1.child-a";
    let call = |tool: &str, args| r.call_sandboxed(tool, args, key).unwrap_or_else(|e| panic!("{tool}: {e}"));
    call("scene_write_spawn_object", json!({"name": "Fresh", "id": "fresh", "parent_id": "root",
        "components": [{"class_name": "Lamp", "properties": {"label": "new"}}]}));
    call("scene_write_set_component", json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 9.0}}));
    call("scene_write_transform", json!({"id": "child", "position": [4, 4, 4]}));
    call("scene_write_delete_object", json!({"id": "child"}));
    key
}

#[test]
fn a_sandbox_edits_a_fork_and_leaves_the_live_scene_alone() {
    let r = rig();
    r.seed();
    let baseline = r.image();
    let key = sandbox_session(&r);

    // Live is untouched and has no transactions to undo.
    r.assert_unchanged_from(&baseline);
    assert!(!r.tb.log().has_undo("run-1"));

    // The sandbox sees its own edits; a live reader does not.
    let in_box = r.call_sandboxed("scene_query", json!({}), key).unwrap();
    let ids: Vec<&str> = in_box["objects"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"fresh") && !ids.contains(&"child"), "{ids:?}");
    let live = r.ok("scene_query", json!({}), "run-1");
    let ids: Vec<&str> = live["objects"].as_array().unwrap().iter().map(|o| o["id"].as_str().unwrap()).collect();
    assert!(ids.contains(&"child") && !ids.contains(&"fresh"), "{ids:?}");

    // A run below the sandbox run stays inside the sandbox.
    r.ok("scene_write_spawn_object", json!({"name": "Deep", "id": "deep"}), "run-1.child-a.grandchild");
    assert!(!r.ids().contains(&"deep".to_string()));

    // The report section is a diff.
    assert!(r.tb.pending().is_empty(), "not pending until the run finishes");
    let sections = r.tb.finish_sandbox(key);
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].0, "scene_diff");
    let text = &sections[0].1;
    assert!(text.contains("+2 added, -1 removed, ~1 changed"), "{text}");
    assert!(text.contains("+ fresh \"Fresh\"") && text.contains("components: Lamp"), "{text}");
    assert!(text.contains("- child \"Child\""), "{text}");
    assert!(text.contains("~ root") && text.contains("Lamp.intensity: 1.0 -> 9.0"), "{text}");
    assert!(text.contains(&format!("review id {key}")), "{text}");
    assert_eq!(r.tb.pending().len(), 1);

    // Finished: the sandbox takes no more writes.
    let err = r
        .call_sandboxed("scene_write_spawn_object", json!({"name": "late"}), key)
        .unwrap_err()
        .to_string();
    assert!(err.contains("sandbox is closed"), "{err}");
}

#[test]
fn applying_a_sandbox_is_one_undoable_transaction() {
    let r = rig();
    r.seed();
    let baseline = r.image();
    let key = sandbox_session(&r);
    r.tb.finish_sandbox(key);

    assert!(r.tb.has_pending_in("run-1") && !r.tb.has_pending_in("run-2"));
    let message = r.tb.apply_sandbox(key, false).unwrap();
    assert!(message.contains("one transaction"), "{message}");
    assert!(r.tb.pending().is_empty());
    assert!(!r.tb.has_pending_in("run-1"));

    let ids = r.ids();
    assert!(ids.contains(&"fresh".to_string()) && !ids.contains(&"child".to_string()), "{ids:?}");
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["components"][0]["data"]["intensity"], 9.0);

    let log = r.tb.log().list_tree("run-1");
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].1, format!("agent:{key}"));
    assert_eq!(log[0].2, "sandbox_apply");

    // The run's revert undoes the applied diff.
    r.tb.revert_tree("run-1").unwrap();
    r.assert_unchanged_from(&baseline);
}

#[test]
fn discarding_a_sandbox_changes_nothing() {
    let r = rig();
    r.seed();
    let baseline = r.image();
    let key = sandbox_session(&r);
    r.tb.finish_sandbox(key);
    r.tb.discard_sandbox(key).unwrap();
    assert!(r.tb.pending().is_empty());
    r.assert_unchanged_from(&baseline);
    assert!(r.tb.apply_sandbox(key, false).is_err());
}

#[test]
fn live_changes_since_the_fork_block_apply_unless_forced() {
    let r = rig();
    r.seed();
    let key = sandbox_session(&r);
    r.tb.finish_sandbox(key);
    // Someone edits the object the sandbox also changed.
    r.ok("scene_write_set_component", json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 3.0}}), "user");
    let after_user = r.image();

    let err = r.tb.apply_sandbox(key, false).unwrap_err();
    assert!(err.contains("'root'") && err.contains("since"), "{err}");
    r.assert_unchanged_from(&after_user);
    assert_eq!(r.tb.pending().len(), 1, "still pending after a refused apply");

    r.tb.apply_sandbox(key, true).unwrap();
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["components"][0]["data"]["intensity"], 9.0, "the sandbox's state won");
}

#[test]
fn live_changes_to_other_objects_do_not_conflict() {
    let r = rig();
    r.seed();
    let key = sandbox_session(&r);
    r.tb.finish_sandbox(key);
    r.ok("scene_write_spawn_object", json!({"name": "Unrelated", "id": "unrelated"}), "user");
    r.tb.apply_sandbox(key, false).unwrap();
    let ids = r.ids();
    assert!(ids.contains(&"unrelated".to_string()) && ids.contains(&"fresh".to_string()));
}

#[test]
fn a_sandbox_that_changed_nothing_reports_that_and_leaves_no_review() {
    let r = rig();
    r.seed();
    let key = "run-1.idle";
    r.call_sandboxed("scene_query", json!({}), key).unwrap();
    let sections = r.tb.finish_sandbox(key);
    assert!(sections[0].1.contains("no scene changes"), "{sections:?}");
    assert!(r.tb.pending().is_empty());
}

#[test]
fn reverting_the_run_tree_discards_its_sandboxes() {
    let r = rig();
    r.seed();
    let key = sandbox_session(&r);
    r.tb.finish_sandbox(key);
    let notes = r.tb.revert_tree("run-1").unwrap();
    assert!(notes.iter().any(|n| n.contains("Discarded 1 scene sandbox")), "{notes:?}");
    assert!(r.tb.pending().is_empty());
}

#[test]
fn the_fork_keeps_ids_and_component_instances() {
    let r = rig();
    r.seed();
    let live_instance = r.ok("scene_get_object", json!({"id": "root"}), "r")["components"][0]["instance_id"].clone();
    let in_box = r
        .call_sandboxed("scene_get_object", json!({"id": "root"}), "run-9.s")
        .unwrap();
    assert_eq!(in_box["components"][0]["instance_id"], live_instance);
    assert_eq!(in_box["components"][0]["data"]["label"], "seed");
}
