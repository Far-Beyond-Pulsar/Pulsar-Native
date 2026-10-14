//! Child runs, the delegation tools and harness loading, against a mock
//! provider that answers by looking at the request it is given.

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
    deliver_pending, project, run_loop, CancelToken, DelegationHost, DelegationLimits, EntryKind,
    EventSink, Harness, HarnessOrigin, HarnessPermissions, HarnessRegistry, Ledger, LedgerEntry,
    ModelPolicy, ModelProfile, PersistedEntry, RunConfig, RunEvent, RunStatus, RunStore,
    SpawnError, SubagentStatus, ROOT_RUN_ID,
};
use serde_json::{json, Value};

const REPORT: &str = "## Summary\nFound the value.\n## Findings\n- x lives in a.rs\n- y is unused\n## Refs\n- a.rs:3\n## Open questions\nnone";

enum Step {
    Say(String),
    Call(&'static str, String, Value),
}

fn say(text: &str) -> Step {
    Step::Say(text.to_string())
}

fn call(name: &'static str, id: &str, args: Value) -> Step {
    Step::Call(name, id.to_string(), args)
}

type Handler = Box<dyn Fn(&ChatRequest) -> Step + Send + Sync>;

struct Router {
    handler: Handler,
    requests: Mutex<Vec<ChatRequest>>,
}

impl Router {
    fn new(handler: impl Fn(&ChatRequest) -> Step + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            handler: Box::new(handler),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests_where(&self, pick: impl Fn(&ChatRequest) -> bool) -> Vec<ChatRequest> {
        self.requests.lock().unwrap().iter().filter(|r| pick(r)).cloned().collect()
    }
}

impl ChatProvider for Router {
    fn id(&self) -> &str {
        "router"
    }

    fn display_name(&self) -> &str {
        "Router"
    }

    fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
        Ok(Vec::new())
    }

    fn chat(&self, request: ChatRequest) -> anyhow::Result<ChatResponse> {
        let step = (self.handler)(&request);
        self.requests.lock().unwrap().push(request);
        let (text, tool_calls) = match step {
            Step::Say(text) => (Some(text), vec![]),
            Step::Call(name, id, args) => (
                None,
                vec![ToolCall {
                    id,
                    name: name.to_string(),
                    arguments_json: args,
                }],
            ),
        };
        Ok(ChatResponse {
            assistant_message: text,
            streamed_text_chunks: vec![],
            tool_calls,
            finish_reason: None,
            raw_response: json!(null),
        })
    }
}

fn is_child(req: &ChatRequest) -> bool {
    req.messages.first().is_some_and(|m| m.content.contains("CHILD"))
}

fn tool_messages(req: &ChatRequest) -> Vec<String> {
    req.messages
        .iter()
        .filter(|m| m.role == ChatRole::Tool)
        .map(|m| m.content.clone())
        .collect()
}

fn tool_names(req: &ChatRequest) -> Vec<String> {
    req.tools.iter().map(|t| t.name.clone()).collect()
}

fn base_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_fn(
        "double",
        "Doubles a number",
        json!({"type": "object", "properties": {"value": {"type": "number"}}}),
        |args, _ctx| Ok(json!({"result": args["value"].as_i64().unwrap_or(0) * 2})),
    );
    registry
}

fn template(provider: Arc<dyn ChatProvider>) -> RunConfig {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());

    let mut config = RunConfig::new(provider, "main-model", base_registry(), agent_runtime::testing::scratch_workspace());
    config.compact_model = "cheap-model".to_string();
    // The harnesses below are read-only; `double` is a pure helper.
    config.safety.classifier.set("double", agent_runtime::ToolClass::Read);
    config.tools = vec![ToolDefinition {
        name: "double".to_string(),
        description: Some("Doubles a number".to_string()),
        parameters_json_schema: json!({"type": "object", "properties": {}}),
    }];
    config
}

fn harnesses(worker_depth: u32) -> HarnessRegistry {
    let mut registry = HarnessRegistry::builtin();
    let worker = Harness::parse(
        &format!(
            "---\nname: worker\ndescription: test worker\ntools: [double]\nmax_steps: 6\nmax_depth: {worker_depth}\n---\nCHILD worker in {{workspace}}.\n{{pinned}}"
        ),
        "worker",
        HarnessOrigin::Project,
    )
    .unwrap();
    registry.insert(worker);
    registry
}

fn collecting_sink() -> (EventSink, Arc<Mutex<Vec<RunEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    (sink, events)
}

fn parent_ledger() -> Ledger {
    let mut ledger = Ledger::with_system_prompt("PARENT system");
    ledger.push(LedgerEntry::user("delegate this"));
    ledger
}

struct Setup {
    host: Arc<DelegationHost>,
    store: Arc<RunStore>,
    host_events: Arc<Mutex<Vec<RunEvent>>>,
}

fn setup(limits: DelegationLimits, worker_depth: u32) -> Setup {
    let store = RunStore::in_memory();
    let (events, host_events) = collecting_sink();
    let host = DelegationHost::new(store.clone(), harnesses(worker_depth), limits, events);
    Setup {
        host,
        store,
        host_events,
    }
}

fn wait(store: &RunStore, id: &str) -> agent_runtime::RunRecord {
    store.wait_terminal(id, &CancelToken::new()).expect("run exists")
}

// ---------------------------------------------------------------------------

#[test]
fn foreground_delegation_returns_the_report_as_a_tool_result() {
    let provider = Router::new(|req| {
        if is_child(req) {
            return if tool_messages(req).is_empty() {
                call("double", "c1", json!({"value": 7}))
            } else {
                say(REPORT)
            };
        }
        let results = tool_messages(req);
        match results.len() {
            0 => call("spawn_subagent", "p1", json!({"harness": "worker", "task": "find x"})),
            1 => {
                let report: Value = serde_json::from_str(&results[0]).unwrap();
                let id = report["run_id"].as_str().unwrap().to_string();
                call("read_run", "p2", json!({"id": id, "range": "0-10"}))
            }
            _ => say("all done"),
        }
    });
    let s = setup(DelegationLimits::default(), 0);
    let mut config = template(provider.clone());
    config.delegation = Some(s.host.root_handle(config.clone()));

    let ledger = run_loop(config, parent_ledger(), CancelToken::new(), collecting_sink().0);

    // The parent saw the report as the result of its own spawn call.
    let exchange = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) if x.call.name == "spawn_subagent" => Some(x.clone()),
            _ => None,
        })
        .expect("spawn_subagent exchange");
    assert!(!exchange.result.is_error, "{}", exchange.result.content);
    let report: Value = serde_json::from_str(&exchange.result.content).unwrap();
    assert_eq!(report["status"], "success");
    assert_eq!(report["summary"], "Found the value.");
    assert_eq!(report["findings"], json!(["x lives in a.rs", "y is unused"]));
    assert_eq!(report["refs"], json!(["a.rs:3"]));
    assert_eq!(report["open_questions"], json!([]));

    // Fields only: nothing in the report is phrased as an instruction.
    let mut keys: Vec<&str> = report.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "findings",
            "harness",
            "open_questions",
            "refs",
            "run_id",
            "status",
            "summary",
            "transcript_entries"
        ]
    );

    // The child's transcript is in the store, not in the parent's ledger.
    let id = report["run_id"].as_str().unwrap();
    let record = s.store.get(id).unwrap();
    assert_eq!(record.status, RunStatus::Success);
    assert_eq!(record.parent.as_deref(), Some(ROOT_RUN_ID));
    assert!(record.transcript.iter().any(|l| l.contains("tool double")));
    assert!(!ledger
        .entries
        .iter()
        .any(|e| matches!(&e.kind, EntryKind::AssistantTurn { text } if text.contains("x lives in a.rs"))));

    // read_run paged the stored transcript back to the parent.
    let read = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) if x.call.name == "read_run" => Some(x.result.content.clone()),
            _ => None,
        })
        .expect("read_run exchange");
    let read: Value = serde_json::from_str(&read).unwrap();
    assert_eq!(read["total_entries"], record.transcript.len());
    assert!(read["entries"].as_array().unwrap().iter().any(|l| l.as_str().unwrap().contains("find x")));

    // The child ran its own loop: its own system prompt, only its own tools,
    // the cheap-or-same model, and no spawn tool (worker has max_depth 0).
    let child_requests = provider.requests_where(is_child);
    assert_eq!(child_requests.len(), 2);
    assert_eq!(child_requests[0].model, "main-model");
    assert_eq!(tool_names(&child_requests[0]), vec!["double"]);
    assert!(child_requests[0].messages[0].content.contains("CHILD worker"));
    assert_eq!(child_requests[0].messages[1].content, "find x");

    // The parent's tool list gained the delegation tools.
    let parent_request = &provider.requests_where(|r| !is_child(r))[0];
    let names = tool_names(parent_request);
    for expected in ["spawn_subagent", "await_subagent", "read_run", "cancel_subagent", "double"] {
        assert!(names.contains(&expected.to_string()), "missing {expected}: {names:?}");
    }
    let spawn_def = parent_request.tools.iter().find(|t| t.name == "spawn_subagent").unwrap();
    assert!(spawn_def.description.as_deref().unwrap().contains("- explorer:"));

    // The card is driven by events.
    let events = s.host_events.lock().unwrap();
    assert!(events.iter().any(|e| matches!(e, RunEvent::ChildSpawned { harness, depth: 1, background: false, .. } if harness == "worker")));
    assert!(events.iter().any(|e| matches!(e, RunEvent::ChildProgress { note, .. } if note.contains("double"))));
    assert!(events.iter().any(|e| matches!(e, RunEvent::ChildFinished { status: SubagentStatus::Success, .. })));
}

#[test]
fn background_completion_is_delivered_as_a_synthetic_exchange() {
    let provider = Router::new(|req| {
        if is_child(req) {
            return say(REPORT);
        }
        if tool_messages(req).is_empty() {
            call(
                "spawn_subagent",
                "p1",
                json!({"harness": "worker", "task": "scan", "background": true}),
            )
        } else {
            say("started a helper")
        }
    });
    let s = setup(DelegationLimits::default(), 0);
    let mut config = template(provider.clone());
    config.delegation = Some(s.host.root_handle(config.clone()));

    let mut ledger = run_loop(config, parent_ledger(), CancelToken::new(), collecting_sink().0);

    // The spawn call returned a handle, not a report.
    let handle: Value = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) => Some(serde_json::from_str(&x.result.content).unwrap()),
            _ => None,
        })
        .unwrap();
    assert_eq!(handle["status"], "running");
    let id = handle["run_id"].as_str().unwrap().to_string();
    assert_eq!(wait(&s.store, &id).status, RunStatus::Success);

    // The panel (parent idle) delivers it without a model request.
    let requests_before = provider.requests.lock().unwrap().len();
    assert_eq!(deliver_pending(&s.store, ROOT_RUN_ID, &mut ledger), 1);
    assert_eq!(deliver_pending(&s.store, ROOT_RUN_ID, &mut ledger), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), requests_before);
    assert!(matches!(ledger.entries.last().unwrap().kind, EntryKind::SubagentReport(_)));

    // Tool-capable model: a check_subagent call plus its tool result.
    ledger.push(LedgerEntry::user("any news?"));
    let messages = project(&ledger, &ModelProfile::default());
    let assistant = messages.iter().find(|m| m.tool_calls.iter().any(|c| c.name == "check_subagent")).unwrap();
    assert_eq!(assistant.role, ChatRole::Assistant);
    let result = messages.iter().find(|m| m.tool_call_id.as_deref() == Some(assistant.tool_calls[0].id.as_str())).unwrap();
    assert_eq!(result.role, ChatRole::Tool);
    assert!(result.content.contains("Found the value."));
    assert!(!messages.iter().any(|m| m.content.contains("<subagent_report")));
    assert_eq!(messages.iter().filter(|m| m.role == ChatRole::System).count(), 1);

    // Tool-less model: the data envelope, in a user turn.
    let messages = project(&ledger, &ModelProfile::new(0, false));
    let envelope = messages.iter().find(|m| m.role != ChatRole::System && m.content.contains("<subagent_report")).unwrap();
    assert_eq!(envelope.role, ChatRole::User);
    assert!(envelope.content.contains("trust=\"data\""));
}

#[test]
fn background_completion_reaches_a_running_parent_before_its_next_request() {
    // Child "slow" finishes while the parent is on its second request.
    let provider = Router::new(|req| {
        if is_child(req) {
            return say(REPORT);
        }
        let results = tool_messages(req);
        if results.is_empty() {
            return call(
                "spawn_subagent",
                "p1",
                json!({"harness": "worker", "task": "scan", "background": true}),
            );
        }
        if results.len() == 1 {
            // Give the child time to finish, then ask for the next step.
            std::thread::sleep(Duration::from_millis(400));
            return call("double", "p2", json!({"value": 1}));
        }
        say("done")
    });
    let s = setup(DelegationLimits::default(), 0);
    let mut config = template(provider.clone());
    config.delegation = Some(s.host.root_handle(config.clone()).deliver_in_run(true));

    run_loop(config, parent_ledger(), CancelToken::new(), collecting_sink().0);

    let last = provider.requests_where(|r| !is_child(r)).pop().unwrap();
    assert!(last
        .messages
        .iter()
        .any(|m| m.tool_calls.iter().any(|c| c.name == "check_subagent")));
}

#[test]
fn cancelling_the_parent_cancels_the_child() {
    let root = CancelToken::new();
    let provider = Router::new({
        let root = root.clone();
        move |req| {
            if is_child(req) {
                // The user presses stop while the child is generating.
                root.cancel();
                return call("double", "c1", json!({"value": 1}));
            }
            call("spawn_subagent", "p1", json!({"harness": "worker", "task": "long job"}))
        }
    });
    let s = setup(DelegationLimits::default(), 0);
    let mut config = template(provider.clone());
    config.delegation = Some(s.host.root_handle(config.clone()));
    let (sink, events) = collecting_sink();

    run_loop(config, parent_ledger(), root, sink);

    let child = s.store.list().pop().unwrap();
    assert_eq!(child.status, RunStatus::Cancelled);
    // The child never started the tool it asked for.
    assert!(!child.transcript.iter().any(|l| l.contains("tool double")));
    assert!(s
        .host_events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, RunEvent::ChildFinished { status: SubagentStatus::Cancelled, .. })));
    // The parent stopped too: one request, no follow-up after the cancel.
    assert_eq!(provider.requests_where(|r| !is_child(r)).len(), 1);
    assert!(matches!(
        events.lock().unwrap().last().unwrap(),
        RunEvent::Finished(Err(message)) if message == agent_runtime::CANCELLED_MESSAGE
    ));
}

#[test]
fn cancel_tokens_cascade_down_but_not_up() {
    let root = CancelToken::new();
    let child = root.child();
    let grandchild = child.child();

    child.cancel();
    assert!(grandchild.is_cancelled());
    assert!(!root.is_cancelled());

    let other = root.child();
    assert!(!other.is_cancelled());
    root.cancel();
    assert!(other.is_cancelled());
}

#[test]
fn cancel_all_stops_queued_and_running_children() {
    let provider = Router::new(|req| {
        std::thread::sleep(Duration::from_millis(50));
        let _ = req;
        say(REPORT)
    });
    let s = setup(
        DelegationLimits {
            max_parallel: 1,
            ..DelegationLimits::default()
        },
        0,
    );
    let handle = s.host.root_handle(template(provider));
    let cancel = CancelToken::new();
    let first = handle.spawn(&cancel, "worker", "a", None, true).unwrap();
    let second = handle.spawn(&cancel, "worker", "b", None, true).unwrap();
    s.host.cancel_all();
    assert_eq!(wait(&s.store, &second).status, RunStatus::Cancelled);
    assert_eq!(wait(&s.store, &first).status, RunStatus::Cancelled);
}

#[test]
fn depth_limit_is_refused() {
    let provider = Router::new(|req| {
        if is_child(req) {
            // A child that is out of depth budget has no spawn tool to call.
            return if tool_messages(req).is_empty() {
                call("spawn_subagent", "c1", json!({"harness": "worker", "task": "deeper"}))
            } else {
                say(REPORT)
            };
        }
        if tool_messages(req).is_empty() {
            call("spawn_subagent", "p1", json!({"harness": "worker", "task": "level one"}))
        } else {
            say("done")
        }
    });
    // One level allowed; the worker harness would allow more, the chat does not.
    let s = setup(
        DelegationLimits {
            max_depth: 1,
            ..DelegationLimits::default()
        },
        5,
    );
    let mut config = template(provider.clone());
    config.delegation = Some(s.host.root_handle(config.clone()));
    run_loop(config, parent_ledger(), CancelToken::new(), collecting_sink().0);

    let child_requests = provider.requests_where(is_child);
    assert!(!tool_names(&child_requests[0]).contains(&"spawn_subagent".to_string()));
    let child = s.store.list().remove(0);
    assert!(child.transcript.iter().any(|l| l.contains("Unknown tool: spawn_subagent")));
    assert_eq!(s.store.len(), 1, "no grandchild was created");

    // Directly: a run with no depth budget is refused with a clear error.
    let zero = setup(
        DelegationLimits {
            max_depth: 0,
            ..DelegationLimits::default()
        },
        0,
    );
    let handle = zero.host.root_handle(template(provider));
    let err = handle
        .spawn(&CancelToken::new(), "worker", "x", None, false)
        .unwrap_err();
    assert_eq!(err, SpawnError::DepthLimit { max_depth: 0 });
    assert!(err.to_string().contains("depth limit"));
    assert!(zero.store.is_empty());
}

#[test]
fn a_nested_child_runs_when_depth_allows() {
    let provider = Router::new(|req| {
        let system = &req.messages[0].content;
        if system.contains("CHILD") && req.messages[1].content == "outer" {
            return if tool_messages(req).is_empty() {
                call("spawn_subagent", "c1", json!({"harness": "worker", "task": "inner"}))
            } else {
                say(REPORT)
            };
        }
        if is_child(req) {
            return say(REPORT);
        }
        if tool_messages(req).is_empty() {
            call("spawn_subagent", "p1", json!({"harness": "worker", "task": "outer"}))
        } else {
            say("done")
        }
    });
    let s = setup(
        DelegationLimits {
            max_depth: 2,
            ..DelegationLimits::default()
        },
        1,
    );
    let mut config = template(provider);
    config.delegation = Some(s.host.root_handle(config.clone()));
    run_loop(config, parent_ledger(), CancelToken::new(), collecting_sink().0);

    let runs = s.store.list();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].depth, 1);
    assert_eq!(runs[1].depth, 2);
    assert_eq!(runs[1].parent.as_deref(), Some(runs[0].id.as_str()));
    assert!(runs.iter().all(|r| r.status == RunStatus::Success));
}

#[test]
fn fan_out_and_total_budgets_are_enforced() {
    let provider = Router::new(|_| say(REPORT));
    let s = setup(
        DelegationLimits {
            max_children_per_run: 2,
            max_total_runs: 3,
            ..DelegationLimits::default()
        },
        0,
    );
    let handle = s.host.root_handle(template(provider));
    let cancel = CancelToken::new();
    handle.spawn(&cancel, "worker", "1", None, true).unwrap();
    handle.spawn(&cancel, "worker", "2", None, true).unwrap();
    assert_eq!(
        handle.spawn(&cancel, "worker", "3", None, true).unwrap_err(),
        SpawnError::FanOut { limit: 2 }
    );
    match handle.spawn(&cancel, "nope", "x", None, true).unwrap_err() {
        SpawnError::UnknownHarness { available, .. } => assert!(available.contains(&"explorer".to_string())),
        other => panic!("{other:?}"),
    }
    assert_eq!(s.store.len(), 2);
}

#[test]
fn parallelism_is_bounded_by_the_limit() {
    for (limit, expected_max) in [(1usize, 1usize), (2, 2)] {
        let current = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let provider = Router::new({
            let current = current.clone();
            let peak = peak.clone();
            move |_| {
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(120));
                current.fetch_sub(1, Ordering::SeqCst);
                say(REPORT)
            }
        });
        let s = setup(
            DelegationLimits {
                max_parallel: limit,
                ..DelegationLimits::default()
            },
            0,
        );
        let handle = s.host.root_handle(template(provider));
        let cancel = CancelToken::new();
        let ids: Vec<String> = (0..2)
            .map(|i| handle.spawn(&cancel, "worker", &format!("t{i}"), None, true).unwrap())
            .collect();
        for id in &ids {
            assert_eq!(wait(&s.store, id).status, RunStatus::Success);
        }
        assert_eq!(peak.load(Ordering::SeqCst), expected_max, "limit {limit}");
    }
}

#[test]
fn a_failing_child_reports_an_error_instead_of_failing_the_parent() {
    struct Failing;
    impl ChatProvider for Failing {
        fn id(&self) -> &str {
            "failing"
        }
        fn display_name(&self) -> &str {
            "Failing"
        }
        fn models(&self) -> anyhow::Result<Vec<ModelDescriptor>> {
            Ok(Vec::new())
        }
        fn chat(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
            Err(anyhow::anyhow!("backend exploded"))
        }
    }
    let s = setup(DelegationLimits::default(), 0);
    let handle = s.host.root_handle(template(Arc::new(Failing)));
    let id = handle.spawn(&CancelToken::new(), "worker", "x", None, true).unwrap();
    let run = wait(&s.store, &id);
    assert_eq!(run.status, RunStatus::Error);
    assert!(run.report.unwrap().summary.contains("backend exploded"));
}

// ---------------------------------------------------------------------------
// Harnesses
// ---------------------------------------------------------------------------

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "agent_runtime_{name}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn project_harnesses_override_global_which_override_builtin() {
    let root = scratch_dir("harness");
    let project = root.join("project");
    let global = root.join("global");
    let project_agents = project.join(".pulsar").join("agents");
    std::fs::create_dir_all(&project_agents).unwrap();
    std::fs::create_dir_all(&global).unwrap();

    std::fs::write(
        global.join("explorer.md"),
        "---\ndescription: global explorer\n---\nGlobal prompt",
    )
    .unwrap();
    std::fs::write(
        global.join("shared.md"),
        "---\nname: shared\ndescription: from global\n---\nGlobal shared",
    )
    .unwrap();
    std::fs::write(
        project_agents.join("explorer.md"),
        "---\nname: explorer\ndescription: project explorer\nmax_steps: 5\n---\nProject prompt for {task}",
    )
    .unwrap();
    std::fs::write(
        project_agents.join("shared.md"),
        "---\nname: shared\ndescription: from project\n---\nProject shared",
    )
    .unwrap();
    std::fs::write(global.join("only_global.md"), "No frontmatter, just a prompt.").unwrap();
    std::fs::write(project_agents.join("broken.md"), "---\nmax_steps: lots\n---\nBody").unwrap();

    let registry = HarnessRegistry::load(Some(&project), Some(&global));

    let explorer = registry.get("explorer").unwrap();
    assert_eq!(explorer.description, "project explorer");
    assert_eq!(explorer.origin, HarnessOrigin::Project);
    assert_eq!(explorer.max_steps, 5);
    assert_eq!(registry.get("shared").unwrap().description, "from project");

    // Global-only entries survive; the file name is the fallback name.
    let only_global = registry.get("only_global").unwrap();
    assert_eq!(only_global.origin, HarnessOrigin::Global);
    assert_eq!(only_global.system_prompt, "No frontmatter, just a prompt.");

    // Untouched built-ins are still there; bad files are reported, not fatal.
    for name in ["planner", "reviewer", "general"] {
        assert_eq!(registry.get(name).unwrap().origin, HarnessOrigin::Builtin);
    }
    assert_eq!(registry.errors.len(), 1);
    assert!(registry.errors[0].0.ends_with("broken.md"));

    // Without a project the global layer still beats the built-in.
    let global_only = HarnessRegistry::load(None, Some(&global));
    assert_eq!(global_only.get("explorer").unwrap().description, "global explorer");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn harness_frontmatter_covers_every_field() {
    let harness = Harness::parse(
        "---\nname: scout\ndescription: \"Finds things: fast\"\ntools:\n  - group:read\n  - web_search\nmodel: cheap  # comment\ncontext_budget: 12_000\nmax_steps: 9\nmax_depth: 1\npermissions: edit-with-checkpoint\nresult: [Summary, Open Questions]\n---\nFind {task} in {workspace}.\n{pinned}",
        "ignored",
        HarnessOrigin::Project,
    )
    .unwrap();
    assert_eq!(harness.name, "scout");
    assert_eq!(harness.description, "Finds things: fast");
    assert_eq!(harness.tools, vec!["group:read", "web_search"]);
    assert_eq!(harness.model_policy, ModelPolicy::Cheap);
    assert_eq!(harness.context_budget_tokens, Some(12000));
    assert_eq!((harness.max_steps, harness.max_depth), (9, 1));
    assert_eq!(harness.permissions, HarnessPermissions::EditWithCheckpoint);
    assert_eq!(harness.result_schema, vec!["summary", "open_questions"]);
    assert_eq!(
        harness.render_prompt("/w", "the bug", "Pinned: x"),
        "Find the bug in /w.\nPinned: x"
    );

    let explicit = Harness::parse("---\nmodel: gpt-x\n---\nbody", "e", HarnessOrigin::Global).unwrap();
    assert_eq!(explicit.model_policy, ModelPolicy::Explicit("gpt-x".to_string()));
    assert!(Harness::parse("---\nname: a\n", "a", HarnessOrigin::Global).is_err());
    assert!(Harness::parse("---\npermissions: root\n---\nbody", "a", HarnessOrigin::Global).is_err());
}

#[test]
fn read_only_harnesses_never_get_edit_tools() {
    let available: Vec<String> = ["list_files", "tree", "call_plugin_tool", "open_file_in_default_editor", "task_list_update"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let registry = HarnessRegistry::builtin();

    let explorer = registry.get("explorer").unwrap();
    assert_eq!(explorer.permissions, HarnessPermissions::ReadOnly);
    let allowed = explorer.resolve_tools(&available);
    assert_eq!(allowed, vec!["list_files", "tree"]);

    // `general` asks for everything and still never gets the UI-only tools.
    let general = registry.get("general").unwrap().resolve_tools(&available);
    assert!(general.contains(&"call_plugin_tool".to_string()));
    assert!(!general.contains(&"open_file_in_default_editor".to_string()));

    // A read-only harness that lists an edit tool by name still does not get it.
    let sneaky = Harness::parse("---\ntools: [call_plugin_tool, tree]\n---\nbody", "s", HarnessOrigin::Project).unwrap();
    assert_eq!(sneaky.resolve_tools(&available), vec!["tree"]);
}

#[test]
fn reports_parse_with_or_without_headings() {
    let schema: Vec<String> = ["summary", "findings", "refs", "open_questions"].iter().map(|s| s.to_string()).collect();
    let report = agent_runtime::parse_report(REPORT, &schema);
    assert_eq!(report.summary, "Found the value.");
    assert_eq!(report.findings.len(), 2);
    assert!(report.open_questions.is_empty());

    let plain = agent_runtime::parse_report("Just prose, no headings.", &schema);
    assert_eq!(plain.summary, "Just prose, no headings.");
    assert!(plain.findings.is_empty());

    let numbered = agent_runtime::parse_report(
        "Preface line\n**Findings:**\n1. first\n2) second\n   continued\n**Open questions**\n- why?",
        &schema,
    );
    assert_eq!(numbered.summary, "Preface line");
    assert_eq!(numbered.findings, vec!["first", "second continued"]);
    assert_eq!(numbered.open_questions, vec!["why?"]);
}

// ---------------------------------------------------------------------------
// Persistence and old chats
// ---------------------------------------------------------------------------

#[test]
fn the_run_store_persists_next_to_the_chat_and_survives_a_restart() {
    let dir = scratch_dir("store");
    let path = RunStore::path_for_chat(&dir, "chat-1");
    assert!(path.to_string_lossy().ends_with("chat-1.runs"));

    // A chat that never delegated has no file and an empty store.
    assert!(RunStore::open(path.clone()).is_empty());

    let provider = Router::new(|_| say(REPORT));
    let store = RunStore::open(path.clone());
    let (events, seen) = collecting_sink();
    let host = DelegationHost::new(store.clone(), harnesses(0), DelegationLimits::default(), events);
    let handle = host.root_handle(template(provider));
    let id = handle.spawn(&CancelToken::new(), "worker", "persist me", None, true).unwrap();
    // `ChildFinished` is sent after the store is written.
    while !seen.lock().unwrap().iter().any(|e| matches!(e, RunEvent::ChildFinished { .. })) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(path.exists());

    // Another process opens the same file: the full run is there.
    let reopened = RunStore::open(path.clone());
    let run = reopened.get(&id).unwrap();
    assert_eq!(run.status, RunStatus::Success);
    assert_eq!(run.report.unwrap().refs, vec!["a.rs:3"]);
    let (lines, total) = reopened.read_transcript(&id, 0, 100).unwrap();
    assert_eq!(lines.len(), total);
    assert!(lines[0].contains("persist me"));

    // A run that was mid-flight when the file was written loads as cancelled
    // and undelivered, so the next idle delivery tells the parent.
    let mut raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    raw["runs"][0]["status"] = json!("running");
    raw["runs"][0]["delivered"] = json!(true);
    std::fs::write(&path, raw.to_string()).unwrap();
    let interrupted = RunStore::open(path);
    let run = interrupted.get(&id).unwrap();
    assert_eq!(run.status, RunStatus::Cancelled);
    assert!(!run.delivered);
    let mut ledger = Ledger::new();
    assert_eq!(deliver_pending(&interrupted, ROOT_RUN_ID, &mut ledger), 1);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn chats_written_before_runs_existed_still_load_and_project() {
    // The shapes older chat files contain: plain roles, and the retired
    // `agent_event` role with its imperative trailer.
    let raw = r#"[
        {"role": "system", "content": "You are an assistant."},
        {"role": "user", "content": "scan the repo"},
        {"role": "assistant", "content": "spawned one"},
        {"role": "agent_event", "content": "Sub-agent 'Search Agent' (id=subagent-1) completed — status: success.\n\nFindings preview:\nthe answer is 42\n\nIntegrate these findings into the current work."},
        {"role": "assistant", "content": "thanks"}
    ]"#;
    let entries: Vec<PersistedEntry> = serde_json::from_str(raw).unwrap();
    let ledger = Ledger::from_persisted(entries);

    assert!(matches!(ledger.entries[3].kind, EntryKind::SubagentReport(_)));
    let native = project(&ledger, &ModelProfile::default());
    let carried = native.iter().find(|m| m.content.contains("the answer is 42")).unwrap();
    assert_eq!(carried.role, ChatRole::Tool);
    assert!(!native.iter().any(|m| m.content.contains("Integrate these findings")));
    assert_eq!(native.iter().filter(|m| m.role == ChatRole::System).count(), 1);

    let plain = project(&ledger, &ModelProfile::new(0, false));
    assert!(plain.iter().any(|m| m.role == ChatRole::User && m.content.contains("<subagent_report")));

    // Saving and loading again keeps the report.
    let again = Ledger::from_persisted(ledger.to_persisted());
    assert!(matches!(again.entries[3].kind, EntryKind::SubagentReport(_)));
}
