//! Phase B2 verification (see `.claude/plans/eager-plotting-lecun.md`,
//! "B2 — `pulsar_reflection` typed `sync_component`"): proves the typed
//! `ComponentRuntimeBehavior::sync_component(component: &Self, ...)` change
//! dispatches correctly end to end -- `#[register_runtime_behavior]`'s
//! generated shim deserializes the `&serde_json::Value` dispatch payload
//! into the registered class's own concrete type exactly once, then calls
//! the real typed `sync_component`, so the component's own body never
//! touches JSON -- the same mechanism every real `helio_component`/
//! `pulsar_physics` component now uses too (see the root `Cargo.toml`'s
//! Pulsar-Reflection `[patch]` comment for that migration's own history).

use engine_class_derive::register_runtime_behavior;
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner, Subsystems,
    apply_runtime_behavior_for_class,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Default, Serialize, Deserialize)]
struct ThrowawayRuntimeComponent {
    value: u32,
}

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for ThrowawayRuntimeComponent {
    const CLASS_NAME: &'static str = "ThrowawayRuntimeComponent";

    fn sync_component(
        owner: &RuntimeComponentOwner,
        _component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        // `component` is a genuinely typed `&Self` here -- no `.as_object()`/
        // `.get(...)` dance in THIS function. The one JSON deserialize that
        // got it here lives in the derive-generated shim, not here.
        if owner.scene_object_id.is_empty() {
            context.report_error("unexpected empty id".to_string());
            return;
        }
        assert_eq!(
            component.value, 7,
            "sync_component must see the real typed value"
        );
    }
}

struct TestContext {
    subsystems: Subsystems,
    project_root: PathBuf,
    errors: Vec<String>,
}

impl ComponentRuntimeContext for TestContext {
    fn subsystems_mut(&mut self) -> &mut Subsystems {
        &mut self.subsystems
    }
    fn project_root(&self) -> &Path {
        &self.project_root
    }
    fn report_error(&mut self, message: String) {
        self.errors.push(message);
    }
}

fn test_context() -> TestContext {
    TestContext {
        subsystems: Subsystems::new(),
        project_root: PathBuf::from("."),
        errors: Vec::new(),
    }
}

fn owner(props: &HashMap<String, serde_json::Value>) -> RuntimeComponentOwner<'_> {
    RuntimeComponentOwner {
        scene_object_id: "throwaway-42",
        position: [0.0; 3],
        rotation: [0.0; 3],
        scale: [1.0; 3],
        props,
    }
}

#[test]
fn typed_sync_component_dispatches_through_inventory_registration() {
    let props = HashMap::new();
    let data = serde_json::to_value(ThrowawayRuntimeComponent { value: 7 }).unwrap();
    let mut ctx = test_context();

    let handled = apply_runtime_behavior_for_class(
        "ThrowawayRuntimeComponent",
        &owner(&props),
        0,
        &data,
        &mut ctx,
    );

    assert!(
        handled,
        "registered class must be found and dispatched via inventory"
    );
    assert!(
        ctx.errors.is_empty(),
        "a correctly-typed dispatch must not report an error"
    );
}

#[test]
fn invalid_json_reports_an_error_not_a_panic() {
    // A caller bug or a genuinely malformed scene file -- either way, a
    // deserialize failure must not panic. The derive-generated shim's
    // `serde_json::from_value` failing gracefully and reporting through
    // `ComponentRuntimeContext::report_error` instead is the whole reason
    // that shim exists rather than an unchecked `.unwrap()`.
    let props = HashMap::new();
    let data = serde_json::json!({ "value": "not a number" });
    let mut ctx = test_context();

    let handled = apply_runtime_behavior_for_class(
        "ThrowawayRuntimeComponent",
        &owner(&props),
        0,
        &data,
        &mut ctx,
    );

    assert!(
        handled,
        "class_name still matches -- dispatch happens, the deserialize inside it fails"
    );
    assert_eq!(ctx.errors.len(), 1);
    assert!(
        ctx.errors[0].contains("ThrowawayRuntimeComponent"),
        "error must name the class: {:?}",
        ctx.errors
    );
}

#[test]
fn unregistered_class_name_is_not_handled() {
    let props = HashMap::new();
    let mut ctx = test_context();

    let handled = apply_runtime_behavior_for_class(
        "NoSuchClass",
        &owner(&props),
        0,
        &serde_json::Value::Null,
        &mut ctx,
    );

    assert!(!handled);
    assert!(ctx.errors.is_empty());
}
