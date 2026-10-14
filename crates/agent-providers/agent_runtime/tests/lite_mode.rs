//! Phase 3 end to end: Lite routing, output caps, the text tool protocol and
//! the 4k-context budget, all through `run_loop` with a scripted provider.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_chat_core::{
    ChatProvider, ChatRequest, ChatResponse, ChatRole, ModelDescriptor, ResponseFormat, ToolCall,
    ToolDefinition,
};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    resolve_profile, run_loop, CancelToken, EntryKind, EventSink, Ledger, LedgerEntry, ModelProfile,
    ResolveInput, RunConfig, RunEvent, TokenEstimator, ToolCalling, FETCH_OUTPUT, MORE_TOOLS,
};
use serde_json::{json, Value};

type Handler = Box<dyn Fn(&ChatRequest, usize) -> ChatResponse + Send + Sync>;

/// Answers each request with `handler(request, index)` and records them.
struct FnProvider {
    handler: Handler,
    requests: Mutex<Vec<ChatRequest>>,
}

impl FnProvider {
    fn new(handler: impl Fn(&ChatRequest, usize) -> ChatResponse + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl ChatProvider for FnProvider {
    fn id(&self) -> &str {
        "fn"
    }
    fn display_name(&self) -> &str {
        "Fn"
    }
    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Vec::new())
    }
    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            requests.len() - 1
        };
        Ok((self.handler)(&request, index))
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

fn calls(calls: Vec<ToolCall>) -> ChatResponse {
    ChatResponse {
        assistant_message: None,
        streamed_text_chunks: vec![],
        tool_calls: calls,
        finish_reason: None,
        raw_response: Value::Null,
    }
}

fn call(name: &str, id: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments_json: args,
    }
}

fn is_summary_request(request: &ChatRequest) -> bool {
    request
        .messages
        .first()
        .is_some_and(|m| m.content.contains("conversation summarizer"))
}

fn init_engine() {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());
}

fn sink() -> (EventSink, Arc<Mutex<Vec<RunEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    (sink, events)
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"path": {"type": "string", "description": "A long winded description of the path argument."}},
        "required": ["path"]
    })
}

/// `n` registry tools, each answering with `output_len` characters.
fn big_registry(names: &[String], output_len: usize) -> (ToolRegistry, Vec<ToolDefinition>) {
    let mut registry = ToolRegistry::new();
    let mut defs = Vec::new();
    for name in names {
        let description = format!(
            "Does the {name} thing. This sentence only pads the description so full schemas are expensive."
        );
        registry.register_fn(name.clone(), description.clone(), schema(), move |_args, _ctx| {
            Ok(json!({ "data": "x".repeat(output_len) }))
        });
        defs.push(ToolDefinition {
            name: name.clone(),
            description: Some(description),
            parameters_json_schema: schema(),
        });
    }
    (registry, defs)
}

fn many_names() -> Vec<String> {
    let mut names: Vec<String> = ["read_file", "write_file", "edit_file", "list_dir", "grep_search", "find_files"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for i in 0..40 {
        names.push(format!("level_editor_op_{i}"));
    }
    names
}

fn run(
    provider: Arc<FnProvider>,
    registry: ToolRegistry,
    defs: Vec<ToolDefinition>,
    profile: ModelProfile,
    ledger: Ledger,
) -> (Ledger, Arc<Mutex<Vec<RunEvent>>>) {
    init_engine();
    let mut config =
        RunConfig::new(provider, "tiny", registry, agent_runtime::testing::scratch_workspace()).with_profile(profile);
    config.tools = defs;
    let (sink, events) = sink();
    let ledger = run_loop(config, ledger, CancelToken::new(), sink);
    (ledger, events)
}

#[test]
fn lite_router_sends_a_small_tool_set_and_more_tools_unlocks_a_category() {
    let (registry, defs) = big_registry(&many_names(), 40);
    assert_eq!(defs.len(), 46);

    let provider = FnProvider::new(|request, index| match index {
        0 => calls(vec![call(MORE_TOOLS, "m1", json!({"category": "scene"}))]),
        _ => {
            let _ = request;
            text("done")
        }
    });
    let mut ledger = Ledger::with_system_prompt("system prompt");
    ledger.push(LedgerEntry::user("read the file src/main.rs and fix the bug"));
    let profile = ModelProfile::new(4_096, true);
    let (ledger, _events) = run(provider.clone(), registry, defs, profile, ledger);

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);

    let first: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    // 5 domain tools plus the more_tools escape hatch.
    assert_eq!(first.len(), 6, "{first:?}");
    assert!(first.contains(&MORE_TOOLS));
    assert!(first.contains(&"read_file"));
    assert!(!first.iter().any(|n| n.starts_with("level_editor")));
    // One-line descriptions, no schema prose.
    let read_file = requests[0].tools.iter().find(|t| t.name == "read_file").unwrap();
    assert_eq!(read_file.description.as_deref(), Some("Does the read_file thing."));
    assert!(!read_file.parameters_json_schema.to_string().contains("long winded"));

    // The unlock shows in the next turn.
    let second: Vec<&str> = requests[1].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(second.iter().filter(|n| n.starts_with("level_editor")).count() >= 3, "{second:?}");

    // The meta call became a normal exchange with a readable answer.
    let answer = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) if x.call.name == MORE_TOOLS => Some(x.result.clone()),
            _ => None,
        })
        .expect("more_tools exchange");
    assert!(!answer.is_error);
    assert!(answer.content.contains("level_editor_op_0"));
}

#[test]
fn lite_profile_swaps_the_system_prompt_and_restores_it() {
    let (registry, defs) = big_registry(&["read_file".to_string()], 10);
    let provider = FnProvider::new(|_, _| text("ok"));
    let long = "You are a very thorough assistant. ".repeat(60);
    let mut ledger = Ledger::with_system_prompt(long.clone());
    ledger.push(LedgerEntry::user("hi"));
    let (ledger, _) = run(provider.clone(), registry, defs, ModelProfile::new(4_096, true), ledger);

    let request = &provider.requests()[0];
    assert_eq!(request.messages[0].role, ChatRole::System);
    assert!(request.messages[0].content.len() < 600, "{}", request.messages[0].content);
    // The caller's prompt is back in the returned ledger.
    assert_eq!(ledger.system_prompt(), Some(long.as_str()));
    // Sampling follows the profile instead of the fixed 0.2 / 8192.
    assert_eq!(request.temperature, Some(0.1));
    assert_eq!(request.max_tokens, Some(1_024));
}

#[test]
fn oversized_tool_output_is_cut_with_a_ref_and_fetchable() {
    let (registry, defs) = big_registry(&["read_file".to_string()], 30_000);
    let provider = FnProvider::new(|request, index| match index {
        0 => calls(vec![call("read_file", "c1", json!({"path": "big.txt"}))]),
        1 => {
            // The result is capped and says how to get the rest.
            let result = request.messages.iter().find(|m| m.role == ChatRole::Tool).unwrap();
            let marker = result.content.find("ref=\"").expect("ref in the cut-off marker") + 5;
            let reference: String = result.content[marker..].chars().take_while(|c| *c != '"').collect();
            calls(vec![call(FETCH_OUTPUT, "c2", json!({"ref": reference, "offset": 100, "length": 50}))])
        }
        _ => text("done"),
    });
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("show me big.txt"));
    let (ledger, events) = run(provider.clone(), registry, defs, ModelProfile::new(4_096, true), ledger);

    let requests = provider.requests();
    assert_eq!(requests.len(), 3);

    let kept = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) if x.call.name == "read_file" => Some(x.result.content.clone()),
            _ => None,
        })
        .unwrap();
    // 4k window: 512-token cap, 2.5 chars per token.
    assert!(kept.chars().count() <= 1_280, "{}", kept.chars().count());
    assert!(kept.contains("omitted"));
    assert!(kept.starts_with("{\"data\":\"xxx"));

    // fetch_output is only offered once there is something to fetch.
    assert!(!requests[0].tools.iter().any(|t| t.name == FETCH_OUTPUT));
    assert!(requests[1].tools.iter().any(|t| t.name == FETCH_OUTPUT));

    let fetched = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) if x.call.name == FETCH_OUTPUT => Some(x.result.clone()),
            _ => None,
        })
        .unwrap();
    assert!(!fetched.is_error, "{}", fetched.content);
    assert!(fetched.content.starts_with("xxxx"));

    // The UI still receives the whole result.
    let events = events.lock().unwrap();
    let full = events
        .iter()
        .find_map(|e| match e {
            RunEvent::ToolCallResult { tool_name, result_full, .. } if tool_name == "read_file" => {
                Some(result_full.len())
            }
            _ => None,
        })
        .unwrap();
    assert!(full > 30_000);
}

fn text_profile() -> ModelProfile {
    let mut profile = ModelProfile::new(8_192, false);
    assert_eq!(profile.tool_calling, ToolCalling::None);
    profile.parallel_tools = false;
    profile
}

#[test]
fn text_protocol_round_trip() {
    let (registry, defs) = big_registry(&["read_file".to_string()], 20);
    let provider = FnProvider::new(|request, index| {
        match index {
            0 => text(
                "I will read it.\n```tool_call\n{\"name\": \"read_file\", \"arguments\": {\"path\": \"a.rs\"}}\n```",
            ),
            1 => {
                // The exchange is rendered back as the same protocol plus data.
                let roles: Vec<ChatRole> = request.messages.iter().map(|m| m.role).collect();
                assert!(!roles.contains(&ChatRole::Tool));
                let assistant = request
                    .messages
                    .iter()
                    .find(|m| m.role == ChatRole::Assistant && m.content.contains("```tool_call"))
                    .expect("the call is shown to the model as text");
                assert!(assistant.content.contains("a.rs"));
                let result = request.messages.last().unwrap();
                assert_eq!(result.role, ChatRole::User);
                assert!(result.content.starts_with("<tool_result name=\"read_file\" trust=\"data\">"));
                text("The file holds x's.")
            }
            _ => unreachable!(),
        }
    });
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("what is in a.rs?"));
    let (ledger, events) = run(provider.clone(), registry, defs, text_profile(), ledger);

    let requests = provider.requests();
    assert_eq!(requests.len(), 2);
    // No structured tools are sent; the protocol is in the system prompt.
    for request in &requests {
        assert!(request.tools.is_empty());
        assert!(!request.enable_tool_calls);
        assert!(request.messages[0].content.contains("```tool_call"));
        assert!(request.messages[0].content.contains("read_file("));
    }

    // The runtime ran the tool and recorded a normal exchange.
    assert!(ledger.entries.iter().any(|e| matches!(
        &e.kind,
        EntryKind::ToolExchange(x) if x.call.name == "read_file" && x.call.arguments_json == json!({"path": "a.rs"})
    )));
    assert!(matches!(
        &ledger.entries.last().unwrap().kind,
        EntryKind::AssistantTurn { text } if text == "The file holds x's."
    ));

    // The raw block never reaches the visible stream.
    let events = events.lock().unwrap();
    let visible: String = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::Chunk(c) => Some(c.as_str()),
            _ => None,
        })
        .collect();
    assert!(visible.contains("I will read it."));
    assert!(!visible.contains("tool_call"), "{visible}");
    assert!(events.iter().any(|e| matches!(e, RunEvent::ToolCallGroup(c) if c.len() == 1)));
}

#[test]
fn text_protocol_reports_malformed_calls_back_to_the_model() {
    let (registry, defs) = big_registry(&["read_file".to_string()], 20);
    let provider = FnProvider::new(|request, index| match index {
        0 => text("```tool_call\n{\"name\": \"nope\", \"arguments\": {}}\n```"),
        1 => {
            let notice = request.messages.last().unwrap();
            assert_eq!(notice.role, ChatRole::User);
            assert!(notice.content.contains("<engine_notice"));
            assert!(notice.content.contains("unknown tool"));
            text("Sorry, answering directly.")
        }
        _ => unreachable!(),
    });
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("go"));
    let (ledger, events) = run(provider, registry, defs, text_profile(), ledger);
    assert!(matches!(
        &ledger.entries.last().unwrap().kind,
        EntryKind::AssistantTurn { text } if text.starts_with("Sorry")
    ));
    assert!(matches!(events.lock().unwrap().last(), Some(RunEvent::Finished(Ok(_)))));
}

#[test]
fn text_protocol_gives_up_after_repeated_garbage() {
    let (registry, defs) = big_registry(&["read_file".to_string()], 20);
    let provider = FnProvider::new(|_, _| text("```tool_call\nnot json\n```"));
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("go"));
    let (_, events) = run(provider.clone(), registry, defs, text_profile(), ledger);
    assert_eq!(provider.requests().len(), 3);
    assert!(matches!(events.lock().unwrap().last(), Some(RunEvent::Finished(Err(_)))));
}

#[test]
fn response_format_reaches_the_request() {
    let (registry, defs) = big_registry(&["read_file".to_string()], 20);
    let provider = FnProvider::new(|_, _| text("ok"));
    init_engine();
    let mut config = RunConfig::new(provider.clone(), "m", registry, agent_runtime::testing::scratch_workspace())
        .with_profile(ModelProfile::new(32_768, true));
    config.tools = defs;
    config.response_format = Some(ResponseFormat::JsonObject);
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("hi"));
    let (sink, _) = sink();
    run_loop(config, ledger, CancelToken::new(), sink);
    assert_eq!(provider.requests()[0].response_format, Some(ResponseFormat::JsonObject));
    // Full profile: every tool is sent as before.
    assert_eq!(provider.requests()[0].tools.len(), 1);
}

#[test]
fn profile_resolution_feeds_the_loop() {
    let profile = resolve_profile(ResolveInput {
        provider_id: "ollama",
        model_id: "llama3.2:3b",
        local: true,
        ..ResolveInput::default()
    });
    assert!(profile.is_lite());
    assert_eq!(profile.json_mode, agent_runtime::JsonMode::Schema);
}

/// A 50-turn conversation on a 4k window: noisy tool output, long answers,
/// repeated compaction. Every request must fit.
fn fifty_turns(provider_usage_factor: Option<f32>) -> (Vec<(usize, usize)>, usize, Ledger) {
    let names = many_names();
    let (registry, defs) = big_registry(&names, 6_000);
    let profile = ModelProfile::new(4_096, true);
    let estimator = TokenEstimator::for_profile(&profile);

    let provider = FnProvider::new(move |request, index| {
        if is_summary_request(request) {
            return text(&"Earlier the user asked many things and the assistant used several tools. ".repeat(12));
        }
        let mut response = if request.messages.iter().rev().take_while(|m| m.role != ChatRole::User).any(|m| m.role == ChatRole::Tool) {
            text(&format!("Answer {index}: {}", "The assistant explains the result in some detail. ".repeat(10)))
        } else {
            calls(vec![call("read_file", &format!("c{index}"), json!({"path": format!("src/file_{index}.rs")}))])
        };
        if let Some(factor) = provider_usage_factor {
            let real = estimator.request(&request.messages, &request.tools) as f32 * factor;
            response.raw_response = json!({"usage": {"prompt_tokens": real as u64, "completion_tokens": 50}});
        }
        response
    });

    init_engine();
    let mut ledger = Ledger::with_system_prompt("You are a thorough engine assistant. ".repeat(80));
    let mut compactions = 0usize;
    for turn in 0..50 {
        ledger.push(LedgerEntry::user(format!(
            "Turn {turn}: please read src/file_{turn}.rs and tell me what it does. {}",
            "Be precise. ".repeat(8)
        )));
        let mut config = RunConfig::new(provider.clone(), "tiny", registry.clone(), agent_runtime::testing::scratch_workspace())
            .with_profile(profile);
        config.tools = defs.clone();
        let (sink, events) = sink();
        ledger = run_loop(config, ledger, CancelToken::new(), sink);
        assert!(
            matches!(events.lock().unwrap().last(), Some(RunEvent::Finished(Ok(_)))),
            "turn {turn} failed"
        );
        compactions += events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, RunEvent::ContextCompacted(_)))
            .count();
    }

    let unit = TokenEstimator::for_profile(&profile);
    let sizes = provider
        .requests()
        .iter()
        .filter(|r| !is_summary_request(r))
        .map(|r| (unit.request(&r.messages, &r.tools), r.max_tokens.unwrap_or(0) as usize))
        .collect();
    (sizes, compactions, ledger)
}

#[test]
fn four_k_profile_stays_under_budget_for_fifty_turns() {
    let (sizes, compactions, ledger) = fifty_turns(None);
    assert!(sizes.len() >= 100, "{}", sizes.len());
    let worst = sizes.iter().map(|(prompt, reply)| prompt + reply).max().unwrap();
    assert!(worst <= 4_096, "worst request {worst} tokens");
    assert!(compactions >= 5, "{compactions}");

    // The conversation kept its shape: one running summary, the first goal and the
    // latest request.
    let summaries = ledger
        .entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Summary { .. }))
        .count();
    assert_eq!(summaries, 1);
    let users: Vec<&str> = ledger
        .entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::UserTurn { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(users.first().unwrap().starts_with("Turn 0:"));
    assert!(users.last().unwrap().starts_with("Turn 49:"));
}

#[test]
fn provider_usage_counts_tighten_the_budget() {
    // The provider says prompts are twice as large as estimated.
    let (honest, _, _) = fifty_turns(None);
    let (strict, _, _) = fifty_turns(Some(2.0));
    let tail = |sizes: &[(usize, usize)]| sizes.iter().rev().take(40).map(|(p, _)| *p).max().unwrap();
    assert!(
        tail(&strict) * 10 < tail(&honest) * 8,
        "strict {} vs honest {}",
        tail(&strict),
        tail(&honest)
    );
}
