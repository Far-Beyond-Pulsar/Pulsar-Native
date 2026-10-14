//! Fork, snapshot and replay.

use agent_chat_core::ToolDefinition;
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    first_divergence, render_call, replay, run_loop, script_from_ledger,
    testing::{MockProvider, ScriptedStep},
    CancelToken, EntryKind, Ledger, LedgerEntry, ModelProfile, PinAuthor, PinKind, RunConfig,
    ToolCalling,
};
use serde_json::json;

use crate::common::{config, init_engine, sink};

fn registry(result: &'static str) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_fn(
        "lookup",
        "Look something up",
        json!({"type": "object", "properties": {}}),
        move |_, _| Ok(json!({ "answer": result })),
    );
    registry
}

fn with_tools(mut run_config: RunConfig, result: &'static str) -> RunConfig {
    run_config.tool_registry = registry(result);
    run_config.tools = vec![ToolDefinition {
        name: "lookup".to_string(),
        description: Some("Look something up".to_string()),
        parameters_json_schema: json!({"type": "object", "properties": {}}),
    }];
    run_config
}

/// Record a two-turn conversation with a tool call in the first turn.
fn record(profile: ModelProfile) -> Ledger {
    init_engine();
    let first = ScriptedStep::call("lookup", "c1", json!({"q": "camera"}));
    let first = if profile.tool_calling == ToolCalling::None {
        ScriptedStep::say(render_call(&first.tool_calls[0]))
    } else {
        first
    };
    let provider = MockProvider::scripted(vec![
        first,
        ScriptedStep::say("It is in src/camera.rs."),
        ScriptedStep::say("Anything else?"),
    ]);
    let mut ledger = Ledger::with_system_prompt("You are the Pulsar assistant.");
    ledger.add_pin(PinKind::Requirement, "Keep answers short.", PinAuthor::User);
    ledger.push(LedgerEntry::user("Where is the camera?"));
    let (s, _e) = sink();
    ledger = run_loop(
        with_tools(config(provider.clone(), profile), "src/camera.rs"),
        ledger,
        CancelToken::new(),
        s.clone(),
    );
    ledger.push(LedgerEntry::user("Thanks."));
    ledger = run_loop(
        with_tools(config(provider, profile), "src/camera.rs"),
        ledger,
        CancelToken::new(),
        s,
    );
    ledger
}

#[test]
fn fork_at_cuts_after_the_entry_and_keeps_current_pins() {
    let mut ledger = record(ModelProfile::new(32_000, true));
    // Pin made after the fork point, plus an edit and a removal.
    let late = match ledger.add_pin(PinKind::Constraint, "No new dependencies.", PinAuthor::User) {
        agent_runtime::PinAdd::Added(ix) => ix,
        other => panic!("{other:?}"),
    };
    let first_user = ledger
        .entries
        .iter()
        .position(|e| matches!(e.kind, EntryKind::UserTurn { .. }))
        .unwrap();

    let fork = ledger.fork_at(first_user);
    // Dialog after the fork point is gone...
    assert!(!fork.entries.iter().any(|e| matches!(e.kind, EntryKind::AssistantTurn { .. })));
    assert!(fork.first_user_text() == Some("Where is the camera?"));
    // ...both pins survived, the late one appended after the cut.
    let texts: Vec<_> = fork.pins().into_iter().map(|p| p.text).collect();
    assert_eq!(texts, vec!["Keep answers short.", "No new dependencies."]);
    assert!(fork.len() < ledger.len());

    // Removing a pin in the source is reflected in later forks.
    assert!(ledger.remove_pin(late));
    assert_eq!(ledger.fork_at(first_user).pins().len(), 1);

    // Forking at the end is a full copy.
    assert_eq!(ledger.fork_at(usize::MAX).len(), ledger.len());
}

#[test]
fn snapshots_restore_exactly() {
    let mut ledger = record(ModelProfile::new(32_000, true));
    let snap = ledger.snapshot();
    ledger.push(LedgerEntry::user("more"));
    ledger.truncate(2);
    ledger.restore(&snap);
    assert_eq!(ledger.len(), snap.len);
    assert_eq!(first_divergence(&ledger, &snap.ledger), None);
}

#[test]
fn replay_reproduces_a_recorded_run() {
    for mode in [ToolCalling::Native, ToolCalling::None] {
        let mut profile = ModelProfile::new(32_000, true);
        profile.tool_calling = mode;
        let recorded = record(profile);
        let (s, _e) = sink();
        let result = replay(
            &recorded,
            &profile,
            |provider| with_tools(config(provider, profile), "src/camera.rs"),
            s,
        );
        assert!(result.reproduced(), "{mode:?}: {:?}", result.divergence);
        assert_eq!(result.requests.len(), 3);
    }
}

#[test]
fn replay_reports_a_divergence_when_a_tool_changes() {
    let profile = ModelProfile::new(32_000, true);
    let recorded = record(profile);
    let (s, _e) = sink();
    let result = replay(
        &recorded,
        &profile,
        |provider| with_tools(config(provider, profile), "somewhere else"),
        s,
    );
    let divergence = result.divergence.expect("tool output changed");
    assert!(divergence.contains("somewhere else"), "{divergence}");
}

#[test]
fn a_script_extracted_from_a_ledger_has_one_step_per_model_response() {
    let recorded = record(ModelProfile::new(32_000, true));
    let steps = script_from_ledger(&recorded, &ModelProfile::new(32_000, true));
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0].tool_calls.len(), 1);
    assert_eq!(steps[1].text, "It is in src/camera.rs.");
}
