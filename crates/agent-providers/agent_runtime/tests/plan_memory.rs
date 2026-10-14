//! Phase 6b: plan mode and project memory, through a scripted provider.

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use agent_chat_core::{
    ChatProvider, ChatRequest, ChatResponse, ChatRole, ModelDescriptor, ToolCall, ToolDefinition,
};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    run_loop, CancelToken, ConfirmPolicy, EventSink, Ledger, LedgerEntry, MemoryContext,
    MemoryInbox, MemoryProposal, MemoryScope, MemoryStore, PinKind, PlanMode, ProposalOrigin,
    RunConfig, EXIT_PLAN_MODE, PLAN_MODE_PROMPT, REMEMBER,
};
use serde_json::{json, Value};

type Handler = Box<dyn Fn(&ChatRequest, usize) -> Vec<ToolCall> + Send + Sync>;

/// Answers each request with the tool calls `handler` picks; no calls ends
/// the run with a plain reply.
struct Scripted {
    handler: Handler,
    requests: Mutex<Vec<ChatRequest>>,
}

impl Scripted {
    fn new(handler: impl Fn(&ChatRequest, usize) -> Vec<ToolCall> + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl ChatProvider for Scripted {
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
        let index = {
            let mut requests = self.requests.lock().unwrap();
            requests.push(request.clone());
            requests.len() - 1
        };
        let calls = (self.handler)(&request, index);
        Ok(ChatResponse {
            assistant_message: calls.is_empty().then(|| "done".to_string()),
            streamed_text_chunks: vec![],
            tool_calls: calls,
            finish_reason: None,
            raw_response: Value::Null,
        })
    }
}

fn call(name: &str, id: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments_json: args,
    }
}

struct Tools {
    registry: ToolRegistry,
    /// Names of the tools that actually ran.
    ran: Arc<Mutex<Vec<String>>>,
}

/// `read_file` (read class), `write_file` and `scene_write_object` (write
/// class), each recording that it ran.
fn tools() -> Tools {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    for name in ["read_file", "write_file", "scene_write_object"] {
        let ran = ran.clone();
        registry.register_fn(
            name,
            name,
            json!({"type": "object", "properties": {}}),
            move |_args, _ctx| {
                ran.lock().unwrap().push(name.to_string());
                Ok(json!({"ok": true}))
            },
        );
    }
    Tools { registry, ran }
}

fn config(provider: Arc<dyn ChatProvider>, tools: &Tools) -> RunConfig {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());

    let mut config = RunConfig::new(provider, "model", tools.registry.clone(), agent_runtime::testing::scratch_workspace());
    config.tools = ["read_file", "write_file", "scene_write_object"]
        .iter()
        .map(|name| ToolDefinition {
            name: name.to_string(),
            description: Some(name.to_string()),
            parameters_json_schema: json!({"type": "object", "properties": {}}),
        })
        .collect();
    config.safety.policy = ConfirmPolicy::Auto;
    config.safety.checkpoints = false;
    config
}

fn silent() -> EventSink {
    Arc::new(|_| {})
}

fn ledger() -> Ledger {
    let mut ledger = Ledger::with_system_prompt("base prompt");
    ledger.push(LedgerEntry::user("add a health bar"));
    ledger
}

fn tool_names(request: &ChatRequest) -> Vec<String> {
    request.tools.iter().map(|t| t.name.clone()).collect()
}

fn system(request: &ChatRequest) -> &str {
    let first = &request.messages[0];
    assert_eq!(first.role, ChatRole::System);
    &first.content
}

fn plan_args() -> Value {
    json!({
        "goal": "Add a health bar",
        "steps": [
            {"title": "Add the component", "detail": "Health on the player", "files": ["src/health.rs"]},
            {"title": "Draw it", "files": ["src/ui.rs"]}
        ],
        "risks": ["Changes the save format"]
    })
}

// ---------------------------------------------------------------------------
// Plan mode

#[test]
fn plan_mode_never_reaches_write_tools_and_ends_with_the_plan() {
    let tools = tools();
    let provider = Scripted::new(|_, index| match index {
        0 => vec![call("read_file", "r1", json!({}))],
        // The model ignores the instructions and tries to edit.
        1 => vec![
            call("write_file", "w1", json!({})),
            call("scene_write_object", "w2", json!({})),
        ],
        2 => vec![call(EXIT_PLAN_MODE, "p1", plan_args())],
        _ => panic!("the run must end once the plan is submitted"),
    });
    let mut cfg = config(provider.clone(), &tools);
    let mode = PlanMode::new();
    cfg.plan_mode = Some(mode.clone());

    let out = run_loop(cfg, ledger(), CancelToken::new(), silent());

    // Write tools never ran, were never advertised, and the plan tool was.
    assert_eq!(*tools.ran.lock().unwrap(), vec!["read_file".to_string()]);
    let requests = provider.requests();
    assert_eq!(requests.len(), 3, "no model turn after exit_plan_mode");
    for request in &requests {
        let names = tool_names(request);
        assert!(names.contains(&"read_file".to_string()));
        assert!(names.contains(&EXIT_PLAN_MODE.to_string()));
        assert!(!names.contains(&"write_file".to_string()));
        assert!(!names.contains(&"scene_write_object".to_string()));
        assert!(system(request).contains(PLAN_MODE_PROMPT));
    }

    // The refused calls came back to the model as errors.
    let refused = out
        .entries
        .iter()
        .filter(|e| matches!(&e.kind, agent_runtime::EntryKind::ToolExchange(x)
            if (x.call.name == "write_file" || x.call.name == "scene_write_object") && x.result.is_error))
        .count();
    assert_eq!(refused, 2);

    // The plan is in the slot, structured, and the prompt is back to normal.
    let plan = mode.take().expect("plan submitted");
    assert_eq!(plan.goal, "Add a health bar");
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.files(), vec!["src/health.rs", "src/ui.rs"]);
    assert_eq!(out.system_prompt(), Some("base prompt"));
}

#[test]
fn a_malformed_plan_is_an_error_the_model_can_fix() {
    let tools = tools();
    let provider = Scripted::new(|_, index| match index {
        0 => vec![call(EXIT_PLAN_MODE, "p1", json!({"goal": "g", "steps": []}))],
        1 => vec![call(EXIT_PLAN_MODE, "p2", plan_args())],
        _ => panic!("ends after the valid plan"),
    });
    let mut cfg = config(provider.clone(), &tools);
    let mode = PlanMode::new();
    cfg.plan_mode = Some(mode.clone());
    run_loop(cfg, ledger(), CancelToken::new(), silent());
    assert_eq!(provider.requests().len(), 2);
    assert!(mode.take().is_some());
}

#[test]
fn an_approved_plan_is_pinned_and_the_edit_run_can_write() {
    let tools = tools();
    // Plan run.
    let provider = Scripted::new(|_, _| vec![call(EXIT_PLAN_MODE, "p1", plan_args())]);
    let mut cfg = config(provider, &tools);
    let mode = PlanMode::new();
    cfg.plan_mode = Some(mode.clone());
    let mut chat = run_loop(cfg, ledger(), CancelToken::new(), silent());
    let plan = mode.take().unwrap();
    assert!(tools.ran.lock().unwrap().is_empty());

    // Approval: the plan (possibly edited) becomes the user's pins, then a
    // normal run starts with every tool.
    let plan = agent_runtime::Plan::from_markdown(&plan.to_markdown()).unwrap();
    assert_eq!(plan.apply_to(&mut chat), 4);
    chat.push(LedgerEntry::user(plan.approval_message()));
    assert_eq!(chat.pins_of(PinKind::Requirement).len(), 1);
    assert_eq!(chat.pins_of(PinKind::Decision).len(), 2);

    let provider = Scripted::new(|_, index| match index {
        0 => vec![call("write_file", "w1", json!({}))],
        _ => vec![],
    });
    let cfg = config(provider.clone(), &tools);
    run_loop(cfg, chat, CancelToken::new(), silent());

    assert_eq!(*tools.ran.lock().unwrap(), vec!["write_file".to_string()]);
    let requests = provider.requests();
    let names = tool_names(&requests[0]);
    assert!(names.contains(&"write_file".to_string()));
    assert!(!names.contains(&EXIT_PLAN_MODE.to_string()));
    let prompt = system(&requests[0]);
    assert!(prompt.contains("Plan step 1/2: Add the component"));
    assert!(prompt.contains("Carry out the approved plan: Add a health bar"));
    assert!(!prompt.contains(PLAN_MODE_PROMPT));
}

// ---------------------------------------------------------------------------
// Project memory

fn store(dir: &tempfile::TempDir) -> MemoryStore {
    MemoryStore::new(dir.path(), Some(&dir.path().join("global")))
}

#[test]
fn memory_loads_into_the_system_prompt_in_stable_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    store.write(MemoryScope::Project, "- Use `just check` before committing.\n").unwrap();
    store.write(MemoryScope::User, "- Keep answers short.\n").unwrap();

    let tools = tools();
    let run = |store: &MemoryStore| {
        let provider = Scripted::new(|_, index| match index {
            0 => vec![call("read_file", "r1", json!({}))],
            _ => vec![],
        });
        let mut cfg = config(provider.clone(), &tools);
        cfg.memory = Some(MemoryContext::new(store.clone(), MemoryInbox::new()));
        let mut chat = ledger();
        chat.add_pin(PinKind::Requirement, "Stay on stable Rust.", agent_runtime::PinAuthor::User);
        let out = run_loop(cfg, chat, CancelToken::new(), silent());
        assert_eq!(out.system_prompt(), Some("base prompt"));
        provider.requests()
    };

    let first = run(&store);
    let second = run(&store);
    assert_eq!(first.len(), 2);

    // Same bytes on every request of a run and across runs: cache-stable.
    let reference = system(&first[0]).to_string();
    assert_eq!(system(&first[1]), reference);
    assert_eq!(system(&second[0]), reference);

    // Fixed position: base prompt, then memory, then the pinned block.
    let base = reference.find("base prompt").unwrap();
    let memory = reference.find("Use `just check` before committing.").unwrap();
    let user_memory = reference.find("Keep answers short.").unwrap();
    let pinned = reference.find("## Pinned context").unwrap();
    assert!(base < memory && memory < user_memory && user_memory < pinned);

    // Editing the file is the only thing that changes the bytes.
    store.write(MemoryScope::Project, "- Use `just check` before committing.\n- New rule.\n").unwrap();
    assert_ne!(system(&run(&store)[0]), reference);
}

#[test]
fn an_empty_memory_adds_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let tools = tools();
    let provider = Scripted::new(|_, _| vec![]);
    let mut cfg = config(provider.clone(), &tools);
    cfg.memory = Some(MemoryContext::new(store(&dir), MemoryInbox::new()));
    run_loop(cfg, ledger(), CancelToken::new(), silent());
    assert_eq!(system(&provider.requests()[0]), "base prompt");
}

#[test]
fn remember_only_proposes_and_the_user_decides() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let inbox = MemoryInbox::new();
    let tools = tools();
    let provider = Scripted::new(|_, index| match index {
        0 => vec![call(REMEMBER, "m1", json!({"text": "Prefer small commits."}))],
        _ => vec![],
    });
    let mut cfg = config(provider, &tools);
    cfg.memory = Some(MemoryContext::new(store.clone(), inbox.clone()));
    // Even with every confirmation off, the tool writes nothing.
    cfg.safety.policy = ConfirmPolicy::Auto;
    run_loop(cfg, ledger(), CancelToken::new(), silent());

    assert_eq!(store.read(MemoryScope::Project), "", "nothing saved without acceptance");
    let pending = inbox.snapshot();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].origin, ProposalOrigin::Agent);
    assert_eq!(pending[0].scope, MemoryScope::Project);

    // Accepting (with an edit) is what writes.
    assert!(store.accept(&pending[0], "Prefer small, focused commits.").unwrap());
    assert!(store.read(MemoryScope::Project).contains("- Prefer small, focused commits."));
    // Accepting the same thing twice does not duplicate it.
    assert!(!store.accept(&pending[0], "prefer small, focused commits.").unwrap());
}

#[test]
fn remember_is_not_offered_to_sub_agents_or_without_memory() {
    let tools = tools();
    let provider = Scripted::new(|_, _| vec![]);
    let cfg = config(provider.clone(), &tools);
    run_loop(cfg, ledger(), CancelToken::new(), silent());
    assert!(!tool_names(&provider.requests()[0]).contains(&REMEMBER.to_string()));
    assert!(agent_runtime::builtin_harnesses()
        .iter()
        .all(|h| !h.tools.iter().any(|t| t == REMEMBER)));
}

#[test]
fn remember_in_plan_mode_survives_the_read_only_trim() {
    let dir = tempfile::tempdir().unwrap();
    let inbox = MemoryInbox::new();
    let tools = tools();
    let provider = Scripted::new(|_, index| match index {
        0 => vec![call(REMEMBER, "m1", json!({"text": "Planning notes go in docs/."}))],
        1 => vec![call(EXIT_PLAN_MODE, "p1", plan_args())],
        _ => panic!("ends after the plan"),
    });
    let mut cfg = config(provider, &tools);
    cfg.plan_mode = Some(PlanMode::new());
    cfg.memory = Some(MemoryContext::new(store(&dir), inbox.clone()));
    run_loop(cfg, ledger(), CancelToken::new(), silent());
    assert_eq!(inbox.len(), 1);
}

#[test]
fn proposals_are_deduplicated_and_capped() {
    let inbox = MemoryInbox::new();
    let proposal = |text: &str| MemoryProposal {
        scope: MemoryScope::Project,
        text: text.to_string(),
        origin: ProposalOrigin::Steward,
        untrusted: false,
    };
    assert!(inbox.push(proposal("Use tabs")));
    assert!(!inbox.push(proposal("use  TABS")));
    assert!(!inbox.push(proposal("   ")));
    for i in 0..40 {
        inbox.push(proposal(&format!("rule {i}")));
    }
    assert_eq!(inbox.len(), agent_runtime_max_pending());
}

fn agent_runtime_max_pending() -> usize {
    20
}

#[test]
fn the_steward_proposes_memory_from_user_words_and_never_applies_it() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let inbox = MemoryInbox::new();
    let provider = Scripted::new(|request, _| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        // The memory request must contain the user's words, never tool output.
        let user = request.messages.last().map(|m| m.content.clone()).unwrap_or_default();
        if user.contains("Recent user messages") {
            assert!(user.contains("always run just check"));
            assert!(!user.contains("SECRET TOOL OUTPUT"));
        }
        vec![]
    });
    // The scripted provider answers prose; give the steward a JSON-speaking one.
    struct Json(Arc<Scripted>);
    impl ChatProvider for Json {
        fn id(&self) -> &str {
            "json"
        }
        fn display_name(&self) -> &str {
            "Json"
        }
        fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
            Ok(Vec::new())
        }
        fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
            let mut response = self.0.chat(request.clone())?;
            let first = request.messages.first().map_or("", |m| m.content.as_str());
            response.assistant_message = Some(if first.contains("project memory file") {
                r#"{"entries":[{"scope":"project","text":"Always run just check before committing."}]}"#
                    .to_string()
            } else {
                r#"{"changes":[]}"#.to_string()
            });
            Ok(response)
        }
    }

    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: agent_runtime::StewardSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    let steward = agent_runtime::Steward::new(
        agent_runtime::StewardConfig {
            provider: Arc::new(Json(provider)),
            model: "steward".to_string(),
            enabled: true,
            drift_check: false,
            json_mode: agent_runtime::JsonMode::None,
        },
        sink,
    );
    steward.enable_memory(agent_runtime::MemoryHook {
        store: store.clone(),
        inbox: inbox.clone(),
    });

    let mut chat = ledger();
    chat.entries.clear();
    chat.push(LedgerEntry::user("always run just check before committing"));
    chat.push(LedgerEntry::tool_exchange(
        call("read_file", "r1", json!({})),
        agent_runtime::ToolResult {
            content: "SECRET TOOL OUTPUT".to_string(),
            is_error: false,
        },
    ));
    steward.after_assistant_turn(&chat, &[]);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while inbox.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let pending = inbox.snapshot();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].origin, ProposalOrigin::Steward);
    assert_eq!(store.read(MemoryScope::Project), "", "proposals are not applied");
}
