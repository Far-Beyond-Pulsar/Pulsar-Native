use std::{path::Path, sync::Arc};

use agent_chat_core::{ChatProvider, ToolCall, ToolDefinition};
use agent_chat_tools::ToolRegistry;

use crate::ledger::{EntryKind, Ledger, LedgerEntry, Source, ToolResult};
use crate::profile::ModelProfile;
use crate::run::{run_loop, CancelToken, EventSink, RunConfig};
use crate::taint::ToolClassifier;
use crate::testing::{scratch_workspace, MockProvider, ScriptedStep};
use crate::time_travel::replay;

fn exchange(id: &str, name: &str, content: String, is_error: bool) -> LedgerEntry {
    LedgerEntry::tool_exchange(
        ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json: serde_json::json!({"url": "https://example.com"}),
        },
        ToolResult { content, is_error },
    )
}

#[test]
fn tool_exchanges_survive_a_round_trip_and_stay_untrusted() {
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("read it"));
    ledger.push(LedgerEntry::assistant(""));
    ledger.push(exchange("c1", "fetch_url", "page".to_string(), true));

    let json = serde_json::to_string(&ledger.to_persisted()).unwrap();
    let back = Ledger::from_persisted(serde_json::from_str(&json).unwrap());
    assert_eq!(back.len(), 4);
    match &back.entries[3].kind {
        EntryKind::ToolExchange(x) => {
            assert_eq!(x.call.id, "c1");
            assert_eq!(x.call.name, "fetch_url");
            assert_eq!(x.call.arguments_json["url"], "https://example.com");
            assert!(x.result.is_error);
            assert_eq!(x.result.content, "page");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        back.entries[3].source,
        Source::Tool {
            name: "fetch_url".to_string()
        }
    );
    // The web result still taints after a reload.
    assert!(back.is_tainted(&ToolClassifier::new(), Path::new(".")));
}

#[test]
fn huge_tool_results_are_trimmed_when_saved() {
    let mut ledger = Ledger::new();
    ledger.push(exchange("c1", "read_file", "x".repeat(200_000), false));
    let saved = ledger.to_persisted();
    assert!(saved[0].content.chars().count() <= crate::persist::MAX_PERSISTED_RESULT_CHARS);
    assert!(saved[0].content.contains("fetch_output"));
}

#[test]
fn a_reloaded_chat_with_tool_exchanges_replays() {
    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register_fn(
            "lookup",
            "Look something up",
            serde_json::json!({"type": "object", "properties": {}}),
            |_, _| Ok(serde_json::json!({"answer": "src/camera.rs"})),
        );
        registry
    }
    fn make(provider: Arc<dyn ChatProvider>) -> RunConfig {
        let mut config = RunConfig::new(provider, "m", registry(), scratch_workspace());
        config.tools = vec![ToolDefinition {
            name: "lookup".to_string(),
            description: Some("Look something up".to_string()),
            parameters_json_schema: serde_json::json!({"type": "object", "properties": {}}),
        }];
        config
    }
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());
    let sink: EventSink = Arc::new(|_| {});

    let provider = MockProvider::scripted(vec![
        ScriptedStep::call("lookup", "c1", serde_json::json!({})),
        ScriptedStep::say("It is in src/camera.rs."),
    ]);
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("where?"));
    let recorded = run_loop(make(provider), ledger, CancelToken::new(), sink.clone());
    assert!(recorded
        .entries
        .iter()
        .any(|e| matches!(e.kind, EntryKind::ToolExchange(_))));

    // Save, reload, replay.
    let json = serde_json::to_string(&recorded.to_persisted()).unwrap();
    let reloaded = Ledger::from_persisted(serde_json::from_str(&json).unwrap());
    let result = replay(&reloaded, &ModelProfile::default(), make, sink);
    assert!(result.reproduced(), "diverged: {:?}", result.divergence);
}
