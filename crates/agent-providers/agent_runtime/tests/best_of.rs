//! Best-of-N through `run_loop` with a mock provider.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use agent_chat_core::{
    ChatProvider, ChatRequest, ChatResponse, ChatRole, ModelDescriptor, ToolCall, ToolDefinition,
};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    run_loop, BestOf, Candidate, CancelToken, EntryKind, EventSink, FnVerifier, Harness,
    HarnessOrigin, Ledger, LedgerEntry, ModelProfile, RunConfig, RunEvent, SchemaVerifier,
    Verdict, Verifier,
};
use serde_json::{json, Value};

type Handler = Box<dyn Fn(&ChatRequest, usize) -> ChatResponse + Send + Sync>;

struct Mock {
    handler: Handler,
    requests: AtomicUsize,
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    delay: Duration,
}

impl Mock {
    fn new(
        delay_ms: u64,
        handler: impl Fn(&ChatRequest, usize) -> ChatResponse + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            requests: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            delay: Duration::from_millis(delay_ms),
        })
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
        let index = self.requests.fetch_add(1, Ordering::SeqCst);
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        let response = (self.handler)(&request, index);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(response)
    }
}

fn text(message: &str) -> ChatResponse {
    ChatResponse {
        assistant_message: Some(message.to_string()),
        streamed_text_chunks: vec![],
        tool_calls: vec![],
        finish_reason: None,
        raw_response: Value::Null,
    }
}

fn tool_call(args: Value) -> ChatResponse {
    ChatResponse {
        assistant_message: None,
        streamed_text_chunks: vec![],
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments_json: args,
        }],
        finish_reason: None,
        raw_response: Value::Null,
    }
}

fn init_engine() {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());
}

fn read_tool() -> (ToolRegistry, Vec<ToolDefinition>, Arc<Mutex<Vec<Value>>>) {
    let schema = json!({
        "type": "object",
        "properties": {"path": {"type": "string"}},
        "required": ["path"]
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    registry.register_fn("read", "Read a file", schema.clone(), {
        let seen = seen.clone();
        move |args, _| {
            seen.lock().unwrap().push(args);
            Ok(json!({"text": "contents"}))
        }
    });
    let defs = vec![ToolDefinition {
        name: "read".into(),
        description: Some("Read a file".into()),
        parameters_json_schema: schema,
    }];
    (registry, defs, seen)
}

struct Outcome {
    ledger: Ledger,
    events: Vec<RunEvent>,
    seen_calls: Vec<Value>,
}

fn run(provider: Arc<Mock>, profile: ModelProfile, best_of: Option<BestOf>) -> Outcome {
    init_engine();
    let (registry, defs, seen) = read_tool();
    let mut config =
        RunConfig::new(provider, "local", registry, PathBuf::from(".")).with_profile(profile);
    config.tools = defs;
    config.best_of = best_of;
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("go"));
    let ledger = run_loop(config, ledger, CancelToken::new(), sink);
    let events = std::mem::take(&mut *events.lock().unwrap());
    let seen_calls = seen.lock().unwrap().clone();
    Outcome {
        ledger,
        events,
        seen_calls,
    }
}

fn native() -> ModelProfile {
    ModelProfile::new(32_768, true)
}

fn good_report() -> Arc<dyn Verifier> {
    Arc::new(FnVerifier::new("has-summary", |c: &Candidate| {
        if c.text.contains("## Summary") {
            Verdict::Accept(1.0)
        } else {
            Verdict::Reject("no summary section".into())
        }
    }))
}

fn final_text(ledger: &Ledger) -> String {
    match &ledger.entries.last().unwrap().kind {
        EntryKind::AssistantTurn { text } => text.clone(),
        other => panic!("{other:?}"),
    }
}

fn reports(events: &[RunEvent]) -> Vec<agent_runtime::BestOfReport> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::BestOf(report) => Some(report.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn off_by_default_costs_one_request() {
    let provider = Mock::new(0, |_, _| text("plain answer"));
    let outcome = run(provider.clone(), native(), None);
    assert_eq!(provider.requests.load(Ordering::SeqCst), 1);
    assert!(reports(&outcome.events).is_empty());
    assert_eq!(final_text(&outcome.ledger), "plain answer");
    // n below 2 is the same as off.
    assert!(BestOf::new(1, good_report()).is_none());
}

#[test]
fn final_report_picks_the_verified_candidate() {
    // Only the third sample is a proper report.
    let provider = Mock::new(0, |_, index| match index {
        2 => text("## Summary\nFound it."),
        _ => text("rambling without structure"),
    });
    let options = BestOf::new(3, good_report()).unwrap();
    let outcome = run(provider.clone(), native(), Some(options));
    assert_eq!(provider.requests.load(Ordering::SeqCst), 3);
    // Parallel sampling: whichever extra request got index 2, that one won.
    assert_eq!(final_text(&outcome.ledger), "## Summary\nFound it.");
    let found = reports(&outcome.events);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].accepted, 1);
    assert!(!found[0].fell_back);
    assert_eq!(found[0].replacement.as_deref(), Some("## Summary\nFound it."));
    assert!(outcome
        .events
        .iter()
        .any(|e| matches!(e, RunEvent::ContextNotice(n) if n.contains("Best-of-3"))));
}

#[test]
fn failing_verifier_falls_back_to_the_first_candidate() {
    let provider = Mock::new(0, |_, index| text(&format!("answer {index}")));
    let reject_all: Arc<dyn Verifier> =
        Arc::new(FnVerifier::new("never", |_: &Candidate| Verdict::Reject("no".into())));
    let outcome = run(provider.clone(), native(), BestOf::new(3, reject_all));
    assert_eq!(provider.requests.load(Ordering::SeqCst), 3);
    assert_eq!(final_text(&outcome.ledger), "answer 0");
    let report = &reports(&outcome.events)[0];
    assert!(report.fell_back);
    assert_eq!(report.chosen, 0);
    assert!(report.replacement.is_none());
    assert!(matches!(outcome.events.last(), Some(RunEvent::Finished(Ok(_)))));
}

#[test]
fn panicking_verifier_does_not_break_the_run() {
    let provider = Mock::new(0, |_, _| text("fine"));
    let boom: Arc<dyn Verifier> =
        Arc::new(FnVerifier::new("boom", |_: &Candidate| -> Verdict { panic!("verifier bug") }));
    let outcome = run(provider, native(), BestOf::new(2, boom));
    assert_eq!(final_text(&outcome.ledger), "fine");
    assert!(reports(&outcome.events)[0].fell_back);
}

#[test]
fn invalid_tool_call_is_resampled_and_valid_ones_cost_nothing() {
    let (_, defs, _) = read_tool();
    let verifier: Arc<dyn Verifier> = Arc::new(SchemaVerifier::new(&defs));

    // The first turn breaks the schema (no `path`); a resample fixes it.
    let provider = Mock::new(0, |request, index| {
        if request.messages.iter().any(|m| m.role == ChatRole::Tool) {
            text("done")
        } else if index == 0 {
            tool_call(json!({"file": "a.rs"}))
        } else {
            tool_call(json!({"path": "a.rs"}))
        }
    });
    let outcome = run(provider.clone(), native(), BestOf::new(3, verifier.clone()));
    assert_eq!(outcome.seen_calls, vec![json!({"path": "a.rs"})]);
    let found = reports(&outcome.events);
    assert_eq!(found.len(), 2, "the bad call turn and the final report");
    assert!(found[0].first_rejection.as_deref().unwrap().contains("path"));
    // 3 samples for the bad turn, then the final text turn (a final report,
    // which the default options sample too).
    assert!(provider.requests.load(Ordering::SeqCst) >= 4);

    // A valid first call is not resampled, so with the final report switched
    // off the whole run is two requests.
    let provider = Mock::new(0, |request, _| {
        if request.messages.iter().any(|m| m.role == ChatRole::Tool) {
            text("done")
        } else {
            tool_call(json!({"path": "a.rs"}))
        }
    });
    let mut options = BestOf::new(3, verifier).unwrap();
    options.on_final_report = false;
    let outcome = run(provider.clone(), native(), Some(options));
    assert_eq!(provider.requests.load(Ordering::SeqCst), 2);
    assert!(reports(&outcome.events).is_empty());
    assert_eq!(outcome.seen_calls.len(), 1);
}

#[test]
fn malformed_text_protocol_call_is_resampled() {
    let (_, defs, _) = read_tool();
    let verifier: Arc<dyn Verifier> = Arc::new(SchemaVerifier::new(&defs));
    let mut profile = ModelProfile::new(8_192, false);
    profile.parallel_tools = false;
    let provider = Mock::new(0, |request, index| {
        if request.messages.iter().any(|m| m.role == ChatRole::User && m.content.starts_with("<tool_result")) {
            text("done")
        } else if index == 0 {
            text("```tool_call\nnot json\n```")
        } else {
            text("```tool_call\n{\"name\": \"read\", \"arguments\": {\"path\": \"b.rs\"}}\n```")
        }
    });
    let mut options = BestOf::new(2, verifier).unwrap();
    options.on_final_report = false;
    let outcome = run(provider, profile, Some(options));
    assert_eq!(outcome.seen_calls, vec![json!({"path": "b.rs"})]);
}

#[test]
fn sampling_respects_profile_parallelism() {
    let sample = |parallel: bool| -> usize {
        let provider = Mock::new(60, |_, index| text(&format!("answer {index}")));
        let mut profile = native();
        profile.parallel_tools = parallel;
        let reject: Arc<dyn Verifier> =
            Arc::new(FnVerifier::new("never", |_: &Candidate| Verdict::Reject("no".into())));
        run(provider.clone(), profile, BestOf::new(4, reject));
        provider.max_in_flight.load(Ordering::SeqCst)
    };
    assert_eq!(sample(false), 1, "serial profile samples one at a time");
    assert!(sample(true) > 1, "parallel profile samples concurrently");
}

#[test]
fn max_parallel_caps_concurrency() {
    let provider = Mock::new(60, |_, index| text(&format!("answer {index}")));
    let reject: Arc<dyn Verifier> =
        Arc::new(FnVerifier::new("never", |_: &Candidate| Verdict::Reject("no".into())));
    let mut options = BestOf::new(5, reject).unwrap();
    options.max_parallel = Some(2);
    run(provider.clone(), native(), Some(options));
    assert_eq!(provider.max_in_flight.load(Ordering::SeqCst), 2);
}

#[test]
fn harness_best_of_option_parses() {
    let source = "---\nname: w\ndescription: d\nbest_of: 4\n---\nPrompt {task}";
    let harness = Harness::parse(source, "w", HarnessOrigin::Project).unwrap();
    assert_eq!(harness.best_of, 4);
    let harness = Harness::parse("---\nname: w\n---\nPrompt", "w", HarnessOrigin::Project).unwrap();
    assert_eq!(harness.best_of, 0);
    assert!(Harness::parse("---\nname: w\nbest_of: many\n---\nP", "w", HarnessOrigin::Project).is_err());
}
