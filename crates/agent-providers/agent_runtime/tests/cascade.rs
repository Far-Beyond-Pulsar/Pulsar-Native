//! Cascade routing end to end with a mock provider that answers by model name.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_chat_core::{
    ChatProvider, ChatRequest, ChatResponse, ModelDescriptor, ToolCall, ToolDefinition,
};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    run_loop, CancelToken, CascadePolicy, CascadeSettings, EntryKind, EscalationReason,
    EscalationScope, EventSink, Ledger, LedgerEntry, ModelProfile, RunConfig, RunEvent,
};
use serde_json::{json, Value};

type Handler = Box<dyn Fn(&ChatRequest, usize) -> anyhow::Result<ChatResponse> + Send + Sync>;

struct Mock {
    handler: Handler,
    models: Mutex<Vec<String>>,
}

impl Mock {
    fn new(
        handler: impl Fn(&ChatRequest, usize) -> anyhow::Result<ChatResponse> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            models: Mutex::new(Vec::new()),
        })
    }

    /// The model of every request, in order.
    fn models(&self) -> Vec<String> {
        self.models.lock().unwrap().clone()
    }
}

impl ChatProvider for Mock {
    fn id(&self) -> &str {
        "mock"
    }
    fn display_name(&self) -> &str {
        "Mock"
    }
    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Vec::new())
    }
    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let index = {
            let mut models = self.models.lock().unwrap();
            models.push(request.model.clone());
            models.len() - 1
        };
        (self.handler)(&request, index)
    }
}

fn text(message: &str) -> anyhow::Result<ChatResponse> {
    Ok(ChatResponse {
        assistant_message: Some(message.to_string()),
        streamed_text_chunks: vec![],
        tool_calls: vec![],
        finish_reason: None,
        raw_response: Value::Null,
    })
}

fn tool_call(name: &str, id: &str, args: Value) -> anyhow::Result<ChatResponse> {
    Ok(ChatResponse {
        assistant_message: None,
        streamed_text_chunks: vec![],
        tool_calls: vec![ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments_json: args,
        }],
        finish_reason: None,
        raw_response: Value::Null,
    })
}

fn init_engine() {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());
}

fn registry() -> (ToolRegistry, Vec<ToolDefinition>) {
    let schema = json!({"type": "object", "properties": {"n": {"type": "integer"}}});
    let mut registry = ToolRegistry::new();
    registry.register_fn("ok", "Works", schema.clone(), |_, _| Ok(json!({"fine": true})));
    registry.register_fn("boom", "Always fails", schema.clone(), |_, _| {
        Err(anyhow::anyhow!("it broke"))
    });
    let defs = ["ok", "boom"]
        .iter()
        .map(|name| ToolDefinition {
            name: name.to_string(),
            description: Some(name.to_string()),
            parameters_json_schema: schema.clone(),
        })
        .collect();
    (registry, defs)
}

fn policy() -> CascadePolicy {
    CascadeSettings {
        enabled: true,
        escalation_model: Some("big".into()),
        ..CascadeSettings::default()
    }
    .policy()
    .expect("enabled with a model")
}

fn run(
    provider: Arc<Mock>,
    profile: ModelProfile,
    cascade: Option<CascadePolicy>,
) -> (Ledger, Vec<RunEvent>) {
    init_engine();
    let (registry, defs) = registry();
    let mut config =
        RunConfig::new(provider, "small", registry, agent_runtime::testing::scratch_workspace()).with_profile(profile);
    config.tools = defs;
    config.cascade = cascade;
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("go"));
    let ledger = run_loop(config, ledger, CancelToken::new(), sink);
    let events = std::mem::take(&mut *events.lock().unwrap());
    (ledger, events)
}

fn native() -> ModelProfile {
    ModelProfile::new(32_768, true)
}

fn text_only() -> ModelProfile {
    let mut profile = ModelProfile::new(8_192, false);
    profile.parallel_tools = false;
    profile
}

fn escalations(events: &[RunEvent]) -> Vec<(EscalationReason, EscalationScope)> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::Escalated(esc) => Some((esc.reason, esc.scope)),
            _ => None,
        })
        .collect()
}

fn final_text(ledger: &Ledger) -> String {
    match &ledger.entries.last().unwrap().kind {
        EntryKind::AssistantTurn { text } => text.clone(),
        other => panic!("last entry is not an assistant turn: {other:?}"),
    }
}

fn notice_shown(events: &[RunEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, RunEvent::ContextNotice(n) if n.contains("Escalated from small to big")))
}

#[test]
fn malformed_tool_calls_escalate() {
    let provider = Mock::new(|request, _| {
        if request.model == "big" {
            text("Handled by the big model.")
        } else {
            text("```tool_call\nnot json\n```")
        }
    });
    let (ledger, events) = run(provider.clone(), text_only(), Some(policy()));
    assert_eq!(provider.models(), ["small", "small", "big"]);
    assert_eq!(
        escalations(&events),
        [(EscalationReason::MalformedToolCalls, EscalationScope::Step)]
    );
    assert!(notice_shown(&events));
    assert_eq!(final_text(&ledger), "Handled by the big model.");
    assert!(matches!(events.last(), Some(RunEvent::Finished(Ok(_)))));
    // The engine notice is in the history the big model read.
    assert!(ledger.entries.iter().any(|e| matches!(
        &e.kind,
        EntryKind::EngineNotice { text } if text.contains("Escalated from small to big")
    )));
}

#[test]
fn tool_failure_streak_escalates_for_a_step_then_the_primary_resumes() {
    let provider = Mock::new(|request, index| {
        if request.model == "big" {
            return tool_call("ok", &format!("b{index}"), json!({"n": index}));
        }
        match index {
            0..=2 => tool_call("boom", &format!("c{index}"), json!({"n": index})),
            _ => text("small finished"),
        }
    });
    let (ledger, events) = run(provider.clone(), native(), Some(policy()));
    let models = provider.models();
    assert_eq!(
        models,
        ["small", "small", "small", "big", "big", "big", "small"]
    );
    assert_eq!(
        escalations(&events),
        [(EscalationReason::ToolFailureStreak, EscalationScope::Step)]
    );
    // After the step budget (three requests) the primary model is back.
    assert_eq!(final_text(&ledger), "small finished");
}

#[test]
fn repeated_identical_calls_are_a_loop() {
    let provider = Mock::new(|request, _| {
        if request.model == "big" {
            text("broke the loop")
        } else {
            tool_call("ok", "c", json!({"n": 1}))
        }
    });
    let (ledger, events) = run(provider.clone(), native(), Some(policy()));
    assert_eq!(provider.models(), ["small", "small", "small", "big"]);
    assert_eq!(
        escalations(&events),
        [(EscalationReason::LoopDetected, EscalationScope::Step)]
    );
    assert_eq!(final_text(&ledger), "broke the loop");
}

#[test]
fn low_confidence_reply_is_redone_by_the_big_model() {
    let provider = Mock::new(|request, _| {
        if request.model == "big" {
            text("Confident answer.")
        } else {
            assert!(
                request.messages[0].content.contains("[low-confidence]"),
                "the primary model is taught the marker"
            );
            text("[low-confidence] probably 42")
        }
    });
    let (ledger, events) = run(provider.clone(), native(), Some(policy()));
    assert_eq!(provider.models(), ["small", "big"]);
    assert_eq!(
        escalations(&events),
        [(EscalationReason::LowConfidence, EscalationScope::Step)]
    );
    assert_eq!(final_text(&ledger), "Confident answer.");
    // The dropped guess is not in the history.
    assert!(!ledger.entries.iter().any(|e| matches!(
        &e.kind,
        EntryKind::AssistantTurn { text } if text.contains("probably 42")
    )));
}

#[test]
fn context_overflow_escalates_for_the_rest_of_the_run() {
    let provider = Mock::new(|request, _| {
        if request.model == "big" {
            text("fits in the big window")
        } else {
            Err(anyhow::anyhow!("This model's maximum context length is 4096 tokens"))
        }
    });
    let (ledger, events) = run(provider.clone(), native(), Some(policy()));
    assert_eq!(provider.models(), ["small", "big"]);
    assert_eq!(
        escalations(&events),
        [(EscalationReason::ContextOverflow, EscalationScope::Run)]
    );
    assert_eq!(final_text(&ledger), "fits in the big window");
}

#[test]
fn run_scope_stays_on_the_escalation_model() {
    let mut policy = policy();
    policy.settings.scope = EscalationScope::Run;
    let provider = Mock::new(|request, index| {
        if request.model == "big" {
            if index < 4 {
                tool_call("ok", &format!("b{index}"), json!({"n": index}))
            } else {
                text("done on big")
            }
        } else {
            tool_call("ok", "c", json!({"n": 1}))
        }
    });
    let (_, events) = run(provider.clone(), native(), Some(policy));
    let models = provider.models();
    assert_eq!(models[3..].iter().filter(|m| *m == "small").count(), 0, "{models:?}");
    assert_eq!(escalations(&events).len(), 1);
}

#[test]
fn escalation_model_gets_its_own_provider() {
    let big = Mock::new(|_, _| text("from the big provider"));
    let small = Mock::new(|_, _| text("[low-confidence] meh"));
    let policy = policy().with_provider(big.clone());
    let (ledger, _) = run(small.clone(), native(), Some(policy));
    assert_eq!(small.models(), ["small"]);
    assert_eq!(big.models(), ["big"]);
    assert_eq!(final_text(&ledger), "from the big provider");
}

#[test]
fn nothing_escalates_when_cascade_is_off() {
    // Off: no policy at all.
    let provider = Mock::new(|request, _| {
        assert_eq!(request.model, "small");
        assert!(
            !request.messages[0].content.contains("[low-confidence]"),
            "no extra prompt text when off"
        );
        text("```tool_call\nnot json\n```")
    });
    let (_, events) = run(provider.clone(), text_only(), None);
    assert_eq!(provider.models(), ["small", "small", "small"]);
    assert!(escalations(&events).is_empty());
    assert!(matches!(events.last(), Some(RunEvent::Finished(Err(_)))));

    // Off: settings with enabled = false give no policy.
    let settings = CascadeSettings {
        enabled: false,
        escalation_model: Some("big".into()),
        ..CascadeSettings::default()
    };
    assert!(settings.policy().is_none());

    // Off: loops and failures just run their course.
    let provider = Mock::new(|request, index| {
        assert_eq!(request.model, "small");
        if index < 5 {
            tool_call("boom", "c", json!({"n": 1}))
        } else {
            text("finished alone")
        }
    });
    let (ledger, events) = run(provider.clone(), native(), None);
    assert_eq!(provider.models().len(), 6);
    assert!(escalations(&events).is_empty());
    assert_eq!(final_text(&ledger), "finished alone");
}
