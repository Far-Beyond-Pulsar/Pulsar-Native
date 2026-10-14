//! Sandbox modes: overlay (recorded ops over a read-through view), copy
//! (capped full fork), off (writes go live), mode resolution, and revert
//! that leaves the user's later edits alone. All on in-memory SceneDB worlds.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_scene_tools::snapshot::full_capture_count;
use agent_scene_tools::{
    resolve_mode, FixedScene, SandboxMode, SceneSettings, SceneSource, SceneToolbox, SharedScene,
    DEFAULT_COPY_MAX_OBJECTS,
};
use common::{rig, Rig};
use serde_json::{json, Value};

const KEY: &str = "run-1.box";

fn set_mode(r: &Rig, mode: SandboxMode, copy_max: usize) {
    r.tb.set_settings(SceneSettings { sandbox_mode: mode, copy_max_objects: copy_max });
}

fn box_call(r: &Rig, tool: &str, args: Value) -> Value {
    r.call_sandboxed(tool, args, KEY).unwrap_or_else(|e| panic!("{tool}: {e}"))
}

fn ids_of(v: &Value) -> Vec<String> {
    v["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["id"].as_str().unwrap().to_string())
        .collect()
}

/// The standard overlay session: add, change, move and delete.
fn edit_in_box(r: &Rig) {
    box_call(r, "scene_write_spawn_object", json!({"name": "Fresh", "id": "fresh", "parent_id": "root",
        "components": [{"class_name": "Lamp", "properties": {"label": "new"}}]}));
    box_call(r, "scene_write_set_component", json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 9.0}}));
    box_call(r, "scene_write_transform", json!({"id": "child", "position": [4, 4, 4]}));
    box_call(r, "scene_write_delete_object", json!({"id": "child"}));
}

// ── Overlay ──────────────────────────────────────────────────────────────────

#[test]
fn overlay_is_the_default_and_diff_apply_round_trips() {
    let r = rig();
    r.seed();
    assert_eq!(r.tb.settings().sandbox_mode, SandboxMode::Overlay);
    let baseline = r.image();

    edit_in_box(&r);
    assert_eq!(r.tb.sandbox_mode_of(KEY), Some(SandboxMode::Overlay));
    r.assert_unchanged_from(&baseline);
    assert!(!r.tb.log().has_undo("run-1"), "the live journal saw nothing");

    let sections = r.tb.finish_sandbox(KEY);
    let text = &sections[0].1;
    assert!(text.contains("+1 added, -1 removed, ~1 changed"), "{text}");
    assert!(text.contains("+ fresh \"Fresh\"") && text.contains("components: Lamp"), "{text}");
    assert!(text.contains("- child \"Child\""), "{text}");
    assert!(text.contains("Lamp.intensity: 1.0 -> 9.0"), "{text}");
    r.assert_unchanged_from(&baseline);

    r.tb.apply_sandbox(KEY, false).unwrap();
    let ids = r.ids();
    assert!(ids.contains(&"fresh".to_string()) && !ids.contains(&"child".to_string()), "{ids:?}");
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["components"][0]["data"]["intensity"], 9.0);
    let log = r.tb.log().list_tree("run-1");
    assert_eq!(log.len(), 1, "one transaction: {log:?}");
    assert_eq!(log[0].2, "sandbox_apply");

    // And the run's revert takes it all back.
    r.tb.revert_tree("run-1").unwrap();
    r.assert_unchanged_from(&baseline);
}

#[test]
fn overlay_discard_changes_nothing() {
    let r = rig();
    r.seed();
    let baseline = r.image();
    edit_in_box(&r);
    r.tb.finish_sandbox(KEY);
    r.tb.discard_sandbox(KEY).unwrap();
    r.assert_unchanged_from(&baseline);
    assert!(r.tb.apply_sandbox(KEY, false).is_err());
    assert!(!r.tb.sandbox_exists(KEY));
}

#[test]
fn overlay_conflicts_block_apply_unless_forced() {
    let r = rig();
    r.seed();
    edit_in_box(&r);
    // The user edits an object the overlay also changed, before the run ends.
    r.ok("scene_write_set_component", json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 3.0}}), "user");
    r.tb.finish_sandbox(KEY);
    let after_user = r.image();

    let err = r.tb.apply_sandbox(KEY, false).unwrap_err();
    assert!(err.contains("'root'"), "{err}");
    r.assert_unchanged_from(&after_user);
    assert_eq!(r.tb.pending().len(), 1, "still pending after a refused apply");

    r.tb.apply_sandbox(KEY, true).unwrap();
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["components"][0]["data"]["intensity"], 9.0, "forced: the overlay wins");
}

#[test]
fn overlay_conflict_when_the_user_changes_after_the_run_finished() {
    let r = rig();
    r.seed();
    edit_in_box(&r);
    r.tb.finish_sandbox(KEY);
    r.ok("scene_write_transform", json!({"id": "child", "position": [7, 7, 7]}), "user");
    let err = r.tb.apply_sandbox(KEY, false).unwrap_err();
    assert!(err.contains("'child'"), "{err}");
    // Unrelated live edits do not conflict.
    let r2 = rig();
    r2.seed();
    edit_in_box(&r2);
    r2.tb.finish_sandbox(KEY);
    r2.ok("scene_write_spawn_object", json!({"name": "Other", "id": "other"}), "user");
    r2.tb.apply_sandbox(KEY, false).unwrap();
}

#[test]
fn the_overlay_child_reads_its_own_pending_edits() {
    let r = rig();
    r.seed();
    let baseline = r.image();
    edit_in_box(&r);

    // The child sees its spawn, its delete, its component edit and its move.
    let all = box_call(&r, "scene_query", json!({}));
    let ids = ids_of(&all);
    assert!(ids.contains(&"fresh".to_string()) && !ids.contains(&"child".to_string()), "{ids:?}");
    assert_eq!(all["scene_object_count"], 2);
    let root = box_call(&r, "scene_get_object", json!({"id": "root"}));
    assert_eq!(root["components"][0]["data"]["intensity"], 9.0);
    assert!(root["children"].to_string().contains("fresh"), "{root}");
    let fresh = box_call(&r, "scene_get_object", json!({"id": "fresh"}));
    assert_eq!(fresh["components"][0]["data"]["label"], "new");

    // A later write validates against the pending state: the deleted object
    // is gone, the spawned one can be edited and parented under.
    let err = r
        .call_sandboxed("scene_write_transform", json!({"id": "child", "position": [1, 1, 1]}), KEY)
        .unwrap_err()
        .to_string();
    assert!(err.contains("child"), "{err}");
    box_call(&r, "scene_write_spawn_object", json!({"name": "Leaf", "id": "leaf", "parent_id": "fresh"}));
    box_call(&r, "scene_write_set_component", json!({"id": "fresh", "class_name": "Lamp", "properties": {"intensity": 5.0}}));
    let leaf = box_call(&r, "scene_get_object", json!({"id": "leaf"}));
    assert_eq!(leaf["parent_id"], "fresh");

    // The live scene saw none of it, and a live reader still sees the original.
    r.assert_unchanged_from(&baseline);
    let live = r.ok("scene_query", json!({}), "someone");
    assert!(ids_of(&live).contains(&"child".to_string()));
    assert!(!ids_of(&live).contains(&"fresh".to_string()));

    // A rejected write leaves no pending op behind.
    assert!(r
        .call_sandboxed("scene_write_add_component", json!({"id": "fresh", "class_name": "NoSuchClass"}), KEY)
        .is_err());
    let text = &r.tb.finish_sandbox(KEY)[0].1;
    assert!(!text.contains("NoSuchClass"), "{text}");
}

#[test]
fn a_run_below_an_overlay_run_stays_in_the_overlay() {
    let r = rig();
    r.seed();
    let baseline = r.image();
    box_call(&r, "scene_write_spawn_object", json!({"name": "A", "id": "a"}));
    r.ok("scene_write_spawn_object", json!({"name": "Deep", "id": "deep"}), "run-1.box.grandchild");
    r.assert_unchanged_from(&baseline);
    let text = &r.tb.finish_sandbox(KEY)[0].1;
    assert!(text.contains("+ deep") && text.contains("+ a "), "{text}");
}

#[test]
fn overlay_cost_does_not_depend_on_the_scene_and_takes_no_snapshot() {
    for size in [4usize, 300] {
        let r = rig();
        r.seed();
        for i in 0..size {
            r.ok("scene_write_spawn_object", json!({"name": format!("Filler {i}"), "id": format!("f{i}")}), "seed");
        }
        let before = full_capture_count();
        box_call(&r, "scene_write_spawn_object", json!({"name": "Fresh", "id": "fresh"}));
        box_call(&r, "scene_write_transform", json!({"id": "f0", "position": [1, 2, 3]}));
        box_call(&r, "scene_query", json!({"limit": 5}));
        r.tb.finish_sandbox(KEY);
        r.tb.apply_sandbox(KEY, false).unwrap();
        assert_eq!(
            full_capture_count(),
            before,
            "an overlay run must not take a whole-scene snapshot (scene of {size} fillers)"
        );
    }

    // The same run in copy mode does snapshot the scene.
    let r = rig();
    r.seed();
    set_mode(&r, SandboxMode::Copy, DEFAULT_COPY_MAX_OBJECTS);
    let before = full_capture_count();
    box_call(&r, "scene_write_spawn_object", json!({"name": "Fresh", "id": "fresh"}));
    assert!(full_capture_count() > before);
}

#[test]
fn an_overlay_that_changed_nothing_leaves_no_review() {
    let r = rig();
    r.seed();
    box_call(&r, "scene_query", json!({}));
    box_call(&r, "scene_write_spawn_object", json!({"name": "T", "id": "t"}));
    box_call(&r, "scene_write_delete_object", json!({"id": "t"}));
    let sections = r.tb.finish_sandbox(KEY);
    assert!(sections[0].1.contains("no scene changes"), "{}", sections[0].1);
    assert!(r.tb.pending().is_empty());
}

// ── Copy ─────────────────────────────────────────────────────────────────────

#[test]
fn copy_mode_forks_the_scene_and_respects_the_cap() {
    assert_eq!(SceneSettings::default().copy_max_objects, 50_000);

    let r = rig();
    r.seed(); // 2 objects
    set_mode(&r, SandboxMode::Copy, 1);
    let err = r
        .call_sandboxed("scene_write_spawn_object", json!({"name": "x"}), KEY)
        .unwrap_err()
        .to_string();
    assert!(err.contains("2 objects") && err.contains("limited to 1"), "{err}");
    assert!(err.contains("copy_max_objects") && err.contains("overlay"), "model-readable: {err}");
    assert!(!r.tb.sandbox_exists(KEY), "a refused fork leaves nothing behind");
    // Reads are refused the same way: the run never silently falls back to live.
    assert!(r.call_sandboxed("scene_query", json!({}), KEY).is_err());

    // The cap is the object count, inclusive.
    set_mode(&r, SandboxMode::Copy, 2);
    box_call(&r, "scene_write_spawn_object", json!({"name": "x", "id": "x"}));
    assert_eq!(r.tb.sandbox_mode_of(KEY), Some(SandboxMode::Copy));
    r.tb.discard_sandbox(KEY).ok();

    // 0 is unlimited.
    set_mode(&r, SandboxMode::Copy, 0);
    box_call(&r, "scene_write_spawn_object", json!({"name": "y", "id": "y"}));
    let text = &r.tb.finish_sandbox(KEY)[0].1;
    assert!(text.contains("+ y"), "{text}");
}

// ── Off ──────────────────────────────────────────────────────────────────────

#[test]
fn off_mode_runs_sandbox_requests_live() {
    let r = rig();
    r.seed();
    set_mode(&r, SandboxMode::Off, DEFAULT_COPY_MAX_OBJECTS);
    let out = box_call(&r, "scene_write_spawn_object", json!({"name": "Live", "id": "live"}));
    assert_eq!(out["transaction"]["sandbox"], false);
    assert!(out["transaction"]["transaction"].is_number(), "journaled: {out}");
    assert!(r.ids().contains(&"live".to_string()), "the write went live");
    assert!(r.tb.log().has_undo("run-1"), "and is revertable like any live edit");
    assert!(!r.tb.sandbox_exists(KEY));
    assert!(r.tb.finish_sandbox(KEY).is_empty(), "no diff section for a live run");
    r.tb.revert_tree("run-1").unwrap();
    assert!(!r.ids().contains(&"live".to_string()));
}

// ── Resolution ───────────────────────────────────────────────────────────────

#[test]
fn mode_resolution_prefers_harness_then_project_then_default() {
    use SandboxMode::*;
    assert_eq!(SceneSettings::default().sandbox_mode, Overlay, "built-in default");
    assert_eq!(resolve_mode(None, Copy), Copy, "project beats the default");
    assert_eq!(resolve_mode(Some("off"), Copy), Off, "harness beats the project");
    assert_eq!(resolve_mode(Some("OVERLAY"), Off), Overlay);
    assert_eq!(resolve_mode(Some("copy"), Overlay), Copy);
    assert_eq!(resolve_mode(Some("true"), Copy), Copy, "plain `true` defers to the project");
    assert_eq!(resolve_mode(Some("nonsense"), Off), Off);
    assert_eq!(Off.next().next().next(), Off);
}

#[test]
fn the_harness_mode_overrides_the_project_in_a_real_call() {
    let r = rig();
    r.seed();
    set_mode(&r, SandboxMode::Off, DEFAULT_COPY_MAX_OBJECTS);
    // Project says off, the harness asks for copy.
    r.call_mode("scene_write_spawn_object", json!({"name": "a", "id": "a"}), KEY, Some("copy")).unwrap();
    assert_eq!(r.tb.sandbox_mode_of(KEY), Some(SandboxMode::Copy));
    assert!(!r.ids().contains(&"a".to_string()));
    r.tb.discard_sandbox(KEY).ok();

    // Project says copy, the harness asks for overlay, another for off.
    set_mode(&r, SandboxMode::Copy, DEFAULT_COPY_MAX_OBJECTS);
    r.call_mode("scene_write_spawn_object", json!({"name": "b", "id": "b"}), KEY, Some("overlay")).unwrap();
    assert_eq!(r.tb.sandbox_mode_of(KEY), Some(SandboxMode::Overlay));
    r.call_mode("scene_write_spawn_object", json!({"name": "c", "id": "c"}), "run-2.live", Some("off")).unwrap();
    assert!(r.ids().contains(&"c".to_string()), "off wrote live");
    // No harness choice: the project's copy applies.
    r.call_mode("scene_write_spawn_object", json!({"name": "d", "id": "d"}), "run-3.box", None).unwrap();
    assert_eq!(r.tb.sandbox_mode_of("run-3.box"), Some(SandboxMode::Copy));
}

#[test]
fn settings_round_trip_through_the_project_file() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(SceneSettings::load(dir.path()), SceneSettings::default());
    let settings = SceneSettings { sandbox_mode: SandboxMode::Copy, copy_max_objects: 123 };
    settings.save(dir.path()).unwrap();
    assert!(dir.path().join(".pulsar").join("agent_scene.json").exists());
    assert_eq!(SceneSettings::load(dir.path()), settings);
    // Partial and invalid files fall back field by field / entirely.
    std::fs::write(SceneSettings::path(dir.path()), r#"{"sandbox_mode":"off"}"#).unwrap();
    let partial = SceneSettings::load(dir.path());
    assert_eq!((partial.sandbox_mode, partial.copy_max_objects), (SandboxMode::Off, 50_000));
    std::fs::write(SceneSettings::path(dir.path()), "{not json").unwrap();
    assert_eq!(SceneSettings::load(dir.path()), SceneSettings::default());
}

// ── Revert leaves the user's later edits alone ───────────────────────────────

#[test]
fn revert_skips_objects_the_user_edited_since_and_force_overrides() {
    let r = rig();
    r.seed();
    // The agent edits root and spawns a lamp.
    r.ok("scene_write_set_component", json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 5.0}}), "run-1");
    r.ok("scene_write_spawn_object", json!({"name": "Mine", "id": "mine"}), "run-1");
    // The user then edits root.
    r.ok("scene_write_set_component", json!({"id": "root", "class_name": "Lamp", "properties": {"intensity": 8.0}}), "user");

    let err = r.tb.revert_tree("run-1").unwrap_err();
    assert!(err.contains("'root'") && err.contains("changed since") && err.contains("force"), "{err}");
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["components"][0]["data"]["intensity"], 8.0, "the user's edit survived");
    assert!(!r.ids().contains(&"mine".to_string()), "the untouched transaction was still undone");
    assert!(r.tb.log().has_undo("run-1"), "the skipped transaction stays revertable");

    let notes = r.tb.revert_tree_with("run-1", true).unwrap();
    assert!(!notes.is_empty());
    let root = r.ok("scene_get_object", json!({"id": "root"}), "r");
    assert_eq!(root["components"][0]["data"]["intensity"], 1.0, "force restored the pre-run value");
    assert!(!r.tb.log().has_undo("run-1"));
}

#[test]
fn revert_of_untouched_edits_still_works_across_several_transactions() {
    let r = rig();
    r.seed();
    let baseline = r.image();
    r.ok("scene_write_transform", json!({"id": "child", "position": [5, 5, 5]}), "run-1");
    r.ok("scene_write_transform", json!({"id": "child", "translate": [1, 0, 0]}), "run-1");
    r.ok("scene_write_delete_object", json!({"id": "child"}), "run-1");
    r.tb.revert_tree("run-1").unwrap();
    r.assert_unchanged_from(&baseline);
}

#[test]
fn reverting_a_deleted_then_user_recreated_object_is_a_conflict() {
    let r = rig();
    r.seed();
    r.ok("scene_write_delete_object", json!({"id": "child"}), "run-1");
    r.ok("scene_write_spawn_object", json!({"name": "Mine now", "id": "child"}), "user");
    let err = r.tb.revert_tree("run-1").unwrap_err();
    assert!(err.contains("'child'"), "{err}");
}

// ── Dirty hook ───────────────────────────────────────────────────────────────

struct Counting {
    inner: FixedScene,
    edits: Arc<AtomicUsize>,
}

impl SceneSource for Counting {
    fn scene(&self) -> Result<SharedScene, String> {
        Ok(self.inner.handle())
    }
    fn scene_edited(&self, _summary: &str) {
        self.edits.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn live_edits_applies_and_reverts_tell_the_host_the_scene_changed() {
    let edits = Arc::new(AtomicUsize::new(0));
    let inner = FixedScene::new();
    let tb = SceneToolbox::new(Arc::new(Counting { inner, edits: edits.clone() }));
    let mut registry = agent_chat_tools::ToolRegistry::new();
    agent_scene_tools::register_tools(&mut registry, &tb);
    let run = |tool: &str, args: Value, id: &str, sandbox: bool| {
        registry.execute(tool, args, &common::scope(id, sandbox)).unwrap()
    };

    run("scene_write_spawn_object", json!({"name": "A", "id": "a"}), "run-1", false);
    assert_eq!(edits.load(Ordering::SeqCst), 1, "a live write marks the level dirty");

    run("scene_write_spawn_object", json!({"name": "B", "id": "b"}), "run-2.box", true);
    run("scene_query", json!({}), "run-2.box", true);
    assert_eq!(edits.load(Ordering::SeqCst), 1, "a sandbox write does not touch the live scene");

    tb.finish_sandbox("run-2.box");
    tb.apply_sandbox("run-2.box", false).unwrap();
    assert_eq!(edits.load(Ordering::SeqCst), 2, "applying does");

    tb.revert_tree("run-1").unwrap();
    assert_eq!(edits.load(Ordering::SeqCst), 3, "and so does a revert");

    // A failed write changes nothing and tells nobody.
    assert!(registry
        .execute("scene_write_delete_object", json!({"id": "nope"}), &common::scope("run-3", false))
        .is_err());
    assert_eq!(edits.load(Ordering::SeqCst), 3);
}
