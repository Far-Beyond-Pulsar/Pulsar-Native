//! Gap fixes: batch taint for children, nested model policy.

use std::sync::{Arc, Mutex};

use agent_chat_core::{ChatRequest, ChatResponse, ChatRole, ToolCall, ToolDefinition};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    run_loop, testing::MockProvider, CancelToken, DelegationHost, DelegationLimits, EventSink,
    Harness, HarnessOrigin, HarnessRegistry, Ledger, LedgerEntry, RunConfig, RunEvent, RunStore,
    ToolClass,
};
use serde_json::{json, Value};

fn init_engine() {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());
}

fn response(text: &str, calls: Vec<ToolCall>) -> anyhow::Result<ChatResponse> {
    Ok(ChatResponse {
        assistant_message: (!text.is_empty()).then(|| text.to_string()),
        streamed_text_chunks: Vec::new(),
        tool_calls: calls,
        finish_reason: None,
        raw_response: Value::Null,
    })
}

fn call(name: &str, id: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments_json: args,
    }
}

fn has_tool_result(req: &ChatRequest) -> bool {
    req.messages.iter().any(|m| m.role == ChatRole::Tool)
}

fn is_child(req: &ChatRequest) -> bool {
    req.messages.first().is_some_and(|m| m.content.contains("CHILD"))
}

fn registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    for name in ["fetch_url", "danger"] {
        registry.register_fn(
            name,
            "test tool",
            json!({"type": "object", "properties": {}}),
            |_, _| Ok(json!({"ok": true})),
        );
    }
    registry
}

fn template(provider: Arc<MockProvider>) -> RunConfig {
    init_engine();
    let mut config = RunConfig::new(
        provider,
        "main-model",
        registry(),
        agent_runtime::testing::scratch_workspace(),
    );
    config.compact_model = "cheap-model".to_string();
    config.safety.classifier.set("fetch_url", ToolClass::External);
    config.tools = ["fetch_url", "danger"]
        .iter()
        .map(|name| ToolDefinition {
            name: name.to_string(),
            description: Some("test tool".to_string()),
            parameters_json_schema: json!({"type": "object", "properties": {}}),
        })
        .collect();
    config
}

fn harness(source: &str, name: &str) -> Harness {
    Harness::parse(source, name, HarnessOrigin::Project).unwrap()
}

fn ledger() -> Ledger {
    let mut ledger = Ledger::with_system_prompt("PARENT");
    ledger.push(LedgerEntry::user("go"));
    ledger
}

#[test]
fn a_child_spawned_beside_an_external_call_starts_tainted() {
    let provider = MockProvider::from_fn(|req, _| {
        if is_child(req) {
            return if has_tool_result(req) {
                response("## Summary\ndone", vec![])
            } else {
                response("", vec![call("danger", "k1", json!({}))])
            };
        }
        if has_tool_result(req) {
            return response("finished", vec![]);
        }
        response(
            "",
            vec![
                call("fetch_url", "p1", json!({"url": "https://example.com"})),
                call("spawn_subagent", "p2", json!({"harness": "writer", "task": "write"})),
            ],
        )
    });
    let mut harnesses = HarnessRegistry::builtin();
    harnesses.insert(harness(
        "---\nname: writer\ndescription: writes\ntools: [danger]\npermissions: edit\n---\nCHILD writer {task}",
        "writer",
    ));
    let confirms: Arc<Mutex<Vec<(String, bool)>>> = Arc::new(Mutex::new(Vec::new()));
    let host_sink: EventSink = {
        let confirms = confirms.clone();
        Arc::new(move |event| {
            if let RunEvent::ConfirmRequest { tool, tainted, reply, .. } = event {
                confirms.lock().unwrap().push((tool, tainted));
                reply.deny();
            }
        })
    };
    let host = DelegationHost::new(
        RunStore::in_memory(),
        harnesses,
        DelegationLimits::default(),
        host_sink,
    );
    let mut config = template(provider);
    config.delegation = Some(host.root_handle(config.clone()));
    let sink: EventSink = Arc::new(|_| {});
    run_loop(config, ledger(), CancelToken::new(), sink);

    let confirms = confirms.lock().unwrap();
    assert_eq!(confirms.len(), 1, "{confirms:?}");
    assert!(confirms[0].0.starts_with("danger"));
    assert!(confirms[0].1, "the child should have inherited the batch's taint");
}

#[test]
fn nested_same_and_cheap_models_resolve_from_the_immediate_parent() {
    let provider = MockProvider::from_fn(|req, _| {
        let system = req.messages.first().map(|m| m.content.clone()).unwrap_or_default();
        if system.contains("CHILD leaf") {
            return response("## Summary\nleaf done", vec![]);
        }
        if system.contains("CHILD mid") {
            return if has_tool_result(req) {
                response("## Summary\nmid done", vec![])
            } else {
                response("", vec![call("spawn_subagent", "m1", json!({"harness": "leaf", "task": "inner"}))])
            };
        }
        if has_tool_result(req) {
            return response("finished", vec![]);
        }
        response("", vec![call("spawn_subagent", "p1", json!({"harness": "mid", "task": "outer"}))])
    });
    let mut harnesses = HarnessRegistry::builtin();
    harnesses.insert(harness(
        "---\nname: mid\ndescription: middle\nmodel: mid-model\nmax_depth: 1\n---\nCHILD mid {task}",
        "mid",
    ));
    harnesses.insert(harness(
        "---\nname: leaf\ndescription: leaf\nmodel: same\n---\nCHILD leaf {task}",
        "leaf",
    ));
    let host = DelegationHost::new(
        RunStore::in_memory(),
        harnesses,
        DelegationLimits::default(),
        Arc::new(|_| {}),
    );
    // A capable model: small ones are not allowed to delegate further.
    host.set_profile_resolver(Some(Arc::new(|_| agent_runtime::ModelProfile::new(64_000, true))));
    let mut config = template(provider.clone());
    config.delegation = Some(host.root_handle(config.clone()));
    let sink: EventSink = Arc::new(|_| {});
    run_loop(config, ledger(), CancelToken::new(), sink);

    let leaf_models: Vec<String> = provider
        .requests()
        .into_iter()
        .filter(|r| r.messages[0].content.contains("CHILD leaf"))
        .map(|r| r.model)
        .collect();
    assert_eq!(leaf_models, vec!["mid-model".to_string()]);
}

const FULL_REPORT: &str = "## Summary\nok\n## Findings\n- a\n## Refs\n- none\n## Open questions\nnone";

fn verify_setup(stubborn: bool) -> (Arc<MockProvider>, Arc<RunStore>, RunConfig) {
    let provider = MockProvider::from_fn(move |req, _| {
        if is_child(req) {
            let told = req
                .messages
                .iter()
                .any(|m| m.content.contains("did not pass verification"));
            return if told && !stubborn {
                response(FULL_REPORT, vec![])
            } else {
                response("just a blurb", vec![])
            };
        }
        if has_tool_result(req) {
            return response("finished", vec![]);
        }
        response("", vec![call("spawn_subagent", "p1", json!({"harness": "checked", "task": "t"}))])
    });
    let mut harnesses = HarnessRegistry::builtin();
    harnesses.insert(harness(
        "---\nname: checked\ndescription: verified\nverify: [schema]\n---\nCHILD checked {task}",
        "checked",
    ));
    let store = RunStore::in_memory();
    let host = DelegationHost::new(store.clone(), harnesses, DelegationLimits::default(), Arc::new(|_| {}));
    let mut config = template(provider.clone());
    config.delegation = Some(host.root_handle(config.clone()));
    (provider, store, config)
}

#[test]
fn a_failing_report_is_sent_back_once_and_then_passes() {
    let (provider, store, config) = verify_setup(false);
    let sink: EventSink = Arc::new(|_| {});
    run_loop(config, ledger(), CancelToken::new(), sink);
    let child_requests = provider.requests().into_iter().filter(is_child).count();
    assert_eq!(child_requests, 2);
    let run = store.list().into_iter().next().unwrap();
    let report = run.report.unwrap();
    assert!(report.unverified.is_none());
    assert_eq!(report.summary, "ok");
}

#[test]
fn a_report_that_keeps_failing_is_returned_flagged_unverified() {
    let (provider, store, config) = verify_setup(true);
    let sink: EventSink = Arc::new(|_| {});
    let ledger = run_loop(config, ledger(), CancelToken::new(), sink);
    // One retry only.
    assert_eq!(provider.requests().into_iter().filter(is_child).count(), 2);
    let run = store.list().into_iter().next().unwrap();
    let report = run.report.clone().unwrap();
    assert!(report.unverified.as_deref().unwrap().contains("missing the section"));
    // The parent sees the flag in the tool result.
    let result = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            agent_runtime::EntryKind::ToolExchange(x) if x.call.name == "spawn_subagent" => {
                Some(x.result.content.clone())
            }
            _ => None,
        })
        .unwrap();
    let value: Value = serde_json::from_str(&result).unwrap();
    assert!(value["unverified"].as_str().is_some());
}
