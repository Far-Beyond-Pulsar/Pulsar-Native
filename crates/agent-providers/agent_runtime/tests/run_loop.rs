use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_chat_core::{
    ChatProvider, ChatRequest, ChatResponse, ChatRole, ModelDescriptor, ToolCall, ToolDefinition,
};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    run_loop, CancelToken, EntryKind, EventSink, Ledger, LedgerEntry, RunConfig, RunEvent,
    CANCELLED_MESSAGE,
};
use serde_json::json;

/// One scripted model turn.
struct Step {
    text: &'static str,
    tool_calls: Vec<ToolCall>,
    /// Runs while the "model" is generating, e.g. to press the stop button.
    during: Option<Box<dyn Fn() + Send + Sync>>,
}

fn say(text: &'static str) -> Step {
    Step {
        text,
        tool_calls: vec![],
        during: None,
    }
}

fn call_tool(name: &str, id: &str) -> Step {
    Step {
        text: "",
        tool_calls: vec![ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json: json!({"value": 7}),
        }],
        during: None,
    }
}

/// Replays `steps` and records every request it receives.
struct ScriptedProvider {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<ChatRequest>>,
}

impl ScriptedProvider {
    fn new(steps: Vec<Step>) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.into()),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl ChatProvider for ScriptedProvider {
    fn id(&self) -> &str {
        "scripted"
    }

    fn display_name(&self) -> &str {
        "Scripted"
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Vec::new())
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        self.requests.lock().unwrap().push(request);
        let step = self
            .steps
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| anyhow::anyhow!("script exhausted"))?;
        if let Some(during) = &step.during {
            during();
        }
        Ok(ChatResponse {
            assistant_message: (!step.text.is_empty()).then(|| step.text.to_string()),
            streamed_text_chunks: vec![],
            tool_calls: step.tool_calls,
            finish_reason: None,
            raw_response: json!(null),
        })
    }
}

fn registry() -> (ToolRegistry, Arc<Mutex<Vec<serde_json::Value>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    registry.register_fn(
        "double",
        "Doubles a number",
        json!({"type": "object", "properties": {"value": {"type": "number"}}}),
        {
            let seen = seen.clone();
            move |args, _ctx| {
                seen.lock().unwrap().push(args.clone());
                let value = args["value"].as_i64().unwrap_or(0);
                Ok(json!({"result": value * 2}))
            }
        },
    );
    (registry, seen)
}

fn config(provider: Arc<dyn ChatProvider>, registry: ToolRegistry) -> RunConfig {
    // The tool runtime state lives in the global engine context.
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());

    let mut config = RunConfig::new(provider, "scripted-model", registry, agent_runtime::testing::scratch_workspace());
    config.tools = vec![ToolDefinition {
        name: "double".to_string(),
        description: Some("Doubles a number".to_string()),
        parameters_json_schema: json!({"type": "object", "properties": {}}),
    }];
    config
}

fn collecting_sink() -> (EventSink, Arc<Mutex<Vec<RunEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    (sink, events)
}

fn start_ledger() -> Ledger {
    let mut ledger = Ledger::with_system_prompt("system prompt");
    ledger.push(LedgerEntry::user("double seven"));
    ledger
}

#[test]
fn executes_a_tool_call_round_trip() {
    let provider = ScriptedProvider::new(vec![call_tool("double", "call-1"), say("The answer is 14.")]);
    let (registry, seen) = registry();
    let (sink, events) = collecting_sink();

    let ledger = run_loop(
        config(provider.clone(), registry),
        start_ledger(),
        CancelToken::new(),
        sink,
    );

    // The tool ran once with the model's arguments.
    assert_eq!(*seen.lock().unwrap(), vec![json!({"value": 7})]);
    assert_eq!(provider.request_count(), 2);

    // The second request carried the call and its result back, in order.
    let second = provider.requests.lock().unwrap()[1].messages.clone();
    let roles: Vec<ChatRole> = second.iter().map(|m| m.role).collect();
    assert_eq!(
        roles,
        vec![
            ChatRole::System,
            ChatRole::User,
            ChatRole::Assistant,
            ChatRole::Tool
        ]
    );
    assert_eq!(second[2].tool_calls[0].id, "call-1");
    assert_eq!(second[3].tool_call_id.as_deref(), Some("call-1"));
    assert!(second[3].content.contains("14"));

    // The ledger holds the whole exchange plus the final answer.
    assert!(ledger
        .entries
        .iter()
        .any(|e| matches!(&e.kind, EntryKind::ToolExchange(x) if x.call.id == "call-1" && !x.result.is_error)));
    assert!(matches!(
        &ledger.entries.last().unwrap().kind,
        EntryKind::AssistantTurn { text } if text == "The answer is 14."
    ));

    // Events: group, then result, then a single Finished(Ok) last.
    let events = events.lock().unwrap();
    let group_at = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolCallGroup(calls) if calls.len() == 1))
        .expect("ToolCallGroup");
    let result_at = events
        .iter()
        .position(|e| matches!(e, RunEvent::ToolCallResult { id, is_error: false, .. } if id == "call-1"))
        .expect("ToolCallResult");
    assert!(group_at < result_at);
    let finished: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, RunEvent::Finished(_)))
        .collect();
    assert_eq!(finished.len(), 1);
    assert!(matches!(events.last(), Some(RunEvent::Finished(Ok(_)))));
}

#[test]
fn unknown_tools_come_back_as_errors_not_panics() {
    let provider = ScriptedProvider::new(vec![call_tool("missing", "call-9"), say("sorry")]);
    let (registry, _) = registry();
    let (sink, events) = collecting_sink();

    let ledger = run_loop(
        config(provider, registry),
        start_ledger(),
        CancelToken::new(),
        sink,
    );

    assert!(ledger
        .entries
        .iter()
        .any(|e| matches!(&e.kind, EntryKind::ToolExchange(x) if x.result.is_error)));
    assert!(events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCallResult { is_error: true, .. })));
}

#[test]
fn cancelled_before_start_makes_no_request() {
    let provider = ScriptedProvider::new(vec![say("never")]);
    let (registry, _) = registry();
    let (sink, events) = collecting_sink();
    let cancel = CancelToken::new();
    cancel.cancel();

    run_loop(config(provider.clone(), registry), start_ledger(), cancel, sink);

    assert_eq!(provider.request_count(), 0);
    match events.lock().unwrap().last() {
        Some(RunEvent::Finished(Err(message))) => assert_eq!(message, CANCELLED_MESSAGE),
        _ => panic!("expected Finished(Err(cancelled))"),
    };
}

#[test]
fn cancelling_mid_run_skips_the_tools_and_stops() {
    let cancel = CancelToken::new();
    let mut first = call_tool("double", "call-1");
    first.during = Some(Box::new({
        let cancel = cancel.clone();
        move || cancel.cancel()
    }));
    let provider = ScriptedProvider::new(vec![first, say("never")]);
    let (registry, seen) = registry();
    let (sink, events) = collecting_sink();

    let ledger = run_loop(config(provider.clone(), registry), start_ledger(), cancel, sink);

    assert_eq!(provider.request_count(), 1);
    assert!(seen.lock().unwrap().is_empty(), "tool must not run after cancel");
    assert!(!ledger
        .entries
        .iter()
        .any(|e| matches!(e.kind, EntryKind::ToolExchange(_))));
    let events = events.lock().unwrap();
    assert!(!events
        .iter()
        .any(|e| matches!(e, RunEvent::ToolCallGroup(_))));
    assert!(matches!(
        events.last(),
        Some(RunEvent::Finished(Err(message))) if message == CANCELLED_MESSAGE
    ));
}

#[test]
fn iteration_limit_ends_the_run() {
    let provider = ScriptedProvider::new(vec![
        call_tool("double", "c1"),
        call_tool("double", "c2"),
        call_tool("double", "c3"),
    ]);
    let (registry, _) = registry();
    let (sink, events) = collecting_sink();
    let mut config = config(provider.clone(), registry);
    config.max_iterations = 2;

    run_loop(config, start_ledger(), CancelToken::new(), sink);

    assert_eq!(provider.request_count(), 2);
    match events.lock().unwrap().last() {
        Some(RunEvent::Finished(Err(message))) => assert!(message.contains("2-iteration limit")),
        _ => panic!("expected Finished(Err(limit))"),
    };
}

#[test]
fn thinking_is_split_from_prose() {
    struct Thinker;
    impl ChatProvider for Thinker {
        fn id(&self) -> &str {
            "thinker"
        }
        fn display_name(&self) -> &str {
            "Thinker"
        }
        fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
            Ok(Vec::new())
        }
        fn chat(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
            Ok(ChatResponse {
                assistant_message: Some("<think>hmm</think>Answer".to_string()),
                streamed_text_chunks: vec!["<think>hmm</think>Answer".to_string()],
                tool_calls: vec![],
                finish_reason: None,
                raw_response: json!(null),
            })
        }
    }

    let (registry, _) = registry();
    let (sink, events) = collecting_sink();
    run_loop(
        config(Arc::new(Thinker), registry),
        start_ledger(),
        CancelToken::new(),
        sink,
    );

    let events = events.lock().unwrap();
    let mut thinking = String::new();
    let mut prose = String::new();
    let mut started = 0;
    let mut done = 0;
    for event in events.iter() {
        match event {
            RunEvent::ThinkingStarted => started += 1,
            RunEvent::ThinkingChunk(text) => thinking.push_str(text),
            RunEvent::ThinkingDone => done += 1,
            RunEvent::Chunk(text) => prose.push_str(text),
            _ => {}
        }
    }
    assert_eq!((started, done), (1, 1));
    assert_eq!(thinking, "hmm");
    assert_eq!(prose, "Answer");
}
