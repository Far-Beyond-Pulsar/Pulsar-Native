//! Run extensions: how a domain crate (the scene tools) joins tool
//! classification, the revert button, child reports and the review list,
//! and the sandbox flag of harnesses. A fake extension stands in for the
//! scene tools here; `agent_scene_tools` tests the real one.

use std::sync::{Arc, Mutex};

use agent_runtime::{
    builtin_harnesses, extension_report_sections, extensions_have_undo, has_checkpoint_tree,
    pending_reviews, register_run_extension, resolve_review, revert_checkpoint_tree, run_in_tree,
    Harness, HarnessOrigin, HarnessPermissions, Review, ReviewDecision, RunExtension, ToolClass,
    ToolClassifier,
};

#[derive(Default)]
struct Fake {
    /// Run ids with something to undo.
    edits: Mutex<Vec<String>>,
    reviews: Mutex<Vec<String>>,
}

impl RunExtension for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn classify(&self, tool: &str) -> Option<ToolClass> {
        match tool {
            "fake_look" | "scene_query" | "scene_get_object" => Some(ToolClass::Read),
            "fake_change" => Some(ToolClass::Write),
            _ => None,
        }
    }

    fn has_undo(&self, run_id: &str) -> bool {
        self.edits.lock().unwrap().iter().any(|r| run_in_tree(r, run_id))
    }

    fn revert(&self, run_id: &str) -> Result<Vec<String>, String> {
        let mut edits = self.edits.lock().unwrap();
        let before = edits.len();
        edits.retain(|r| !run_in_tree(r, run_id));
        Ok(vec![format!("Undid {} fake edit(s).", before - edits.len())])
    }

    fn report_sections(&self, run_id: &str) -> Vec<(String, String)> {
        if run_id.ends_with(".sandboxed") {
            vec![("fake_diff".to_string(), "+1 thing".to_string())]
        } else {
            Vec::new()
        }
    }

    fn pending_reviews(&self) -> Vec<Review> {
        self.reviews
            .lock()
            .unwrap()
            .iter()
            .map(|id| Review {
                id: id.clone(),
                run_id: id.clone(),
                title: format!("review {id}"),
                summary: "diff".to_string(),
            })
            .collect()
    }

    fn resolve_review(&self, id: &str, decision: ReviewDecision) -> Result<String, String> {
        self.reviews.lock().unwrap().retain(|r| r != id);
        Ok(format!("{decision:?} {id}"))
    }
}

#[test]
fn an_extension_joins_classification_revert_reports_and_reviews() {
    let fake = Arc::new(Fake::default());
    register_run_extension(fake.clone());
    let root = tempfile::tempdir().unwrap();

    // Tool classes come from the extension; everything else keeps its default.
    let classifier = ToolClassifier::default();
    assert_eq!(classifier.classify("fake_look"), ToolClass::Read);
    assert_eq!(classifier.classify("fake_change"), ToolClass::Write);
    assert_eq!(classifier.classify("list_files"), ToolClass::Read);
    assert_eq!(classifier.classify("web_search"), ToolClass::External);
    assert_eq!(classifier.classify("unknown_tool"), ToolClass::Write);
    // An explicit override still wins.
    let overridden = ToolClassifier::default().with_override("fake_change", ToolClass::Read);
    assert_eq!(overridden.classify("fake_change"), ToolClass::Read);

    // Revert: no file checkpoint exists, only the extension has something.
    assert!(!extensions_have_undo("run-1"));
    assert!(!has_checkpoint_tree(root.path(), "run-1"));
    assert!(revert_checkpoint_tree(root.path(), "run-1").is_err());

    fake.edits.lock().unwrap().extend(["run-1".to_string(), "run-1.c1".to_string(), "run-2".to_string()]);
    assert!(extensions_have_undo("run-1"));
    assert!(has_checkpoint_tree(root.path(), "run-1"));
    let report = revert_checkpoint_tree(root.path(), "run-1").unwrap();
    assert!(report.is_clean());
    assert_eq!(report.extension_notes, vec!["Undid 2 fake edit(s).".to_string()]);
    assert!(report.summary().contains("Undid 2 fake edit(s)."), "{}", report.summary());
    assert!(!has_checkpoint_tree(root.path(), "run-1"));
    assert!(has_checkpoint_tree(root.path(), "run-2"), "other runs keep theirs");

    // Report sections for a finished child.
    assert!(extension_report_sections("run-1.c1").is_empty());
    assert_eq!(
        extension_report_sections("run-1.sandboxed"),
        vec![("fake_diff".to_string(), "+1 thing".to_string())]
    );

    // Reviews.
    fake.reviews.lock().unwrap().push("rv-1".to_string());
    assert_eq!(pending_reviews().len(), 1);
    assert_eq!(resolve_review("rv-1", ReviewDecision::Apply).unwrap(), "Apply rv-1");
    assert!(pending_reviews().is_empty());
    assert!(resolve_review("rv-1", ReviewDecision::Discard).is_err());
}

#[test]
fn sandbox_frontmatter_accepts_a_mode() {
    let parse = |value: &str| {
        Harness::parse(
            &format!("---\nname: b\ndescription: d\nsandbox: {value}\n---\nPrompt"),
            "b",
            HarnessOrigin::Project,
        )
        .unwrap()
    };
    for (value, sandbox, mode) in [
        ("true", true, None),
        ("false", false, None),
        ("overlay", true, Some("overlay")),
        ("Copy", true, Some("copy")),
        ("off", true, Some("off")),
    ] {
        let h = parse(value);
        assert_eq!((h.sandbox, h.sandbox_mode.as_deref()), (sandbox, mode), "sandbox: {value}");
    }
    // The built-in scene-sandbox harness leaves the mode to the project.
    let builtins = builtin_harnesses();
    let boxed = builtins.iter().find(|h| h.name == "scene-sandbox").unwrap();
    assert!(boxed.sandbox && boxed.sandbox_mode.is_none());
}

#[test]
fn sandbox_is_a_harness_setting() {
    let parsed = Harness::parse(
        "---\nname: boxed\ndescription: d\ntools: [group:scene]\npermissions: engine-scene-write\nsandbox: true\n---\nPrompt {task}",
        "boxed",
        HarnessOrigin::Project,
    )
    .unwrap();
    assert!(parsed.sandbox);
    assert_eq!(parsed.permissions, HarnessPermissions::EngineSceneWrite);

    let plain = Harness::parse("---\nname: p\ndescription: d\n---\nPrompt", "p", HarnessOrigin::Project).unwrap();
    assert!(!plain.sandbox);

    let builtins = builtin_harnesses();
    let sandbox = builtins.iter().find(|h| h.name == "scene-sandbox").expect("built-in sandbox harness");
    assert!(sandbox.sandbox);
    assert_eq!(sandbox.permissions, HarnessPermissions::EngineSceneWrite);
    assert_eq!(sandbox.tools, vec!["group:read".to_string(), "group:scene".to_string()]);
    let editor = builtins.iter().find(|h| h.name == "scene-editor").unwrap();
    assert!(!editor.sandbox);
    assert!(builtins.iter().filter(|h| h.sandbox).count() == 1);
}

#[test]
fn scene_tool_names_follow_the_harness_permission_rules() {
    let available: Vec<String> = [
        "list_files",
        "scene_query",
        "scene_get_object",
        "scene_write_spawn_object",
        "scene_write_delete_object",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let harness = |permissions: &str| {
        Harness::parse(
            &format!("---\nname: h\ndescription: d\ntools: [group:read, group:scene]\npermissions: {permissions}\n---\nP"),
            "h",
            HarnessOrigin::Project,
        )
        .unwrap()
    };
    // Read-only and edit-with-checkpoint never get the scene writes.
    for permissions in ["read-only", "edit-with-checkpoint"] {
        let tools = harness(permissions).resolve_tools(&available);
        assert!(tools.contains(&"scene_query".to_string()), "{permissions}: {tools:?}");
        assert!(!tools.iter().any(|t| t.starts_with("scene_write")), "{permissions}: {tools:?}");
    }
    let tools = harness("engine-scene-write").resolve_tools(&available);
    assert!(tools.contains(&"scene_write_spawn_object".to_string()), "{tools:?}");
}
