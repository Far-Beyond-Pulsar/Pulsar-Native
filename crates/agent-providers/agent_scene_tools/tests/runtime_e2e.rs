//! The scene tools inside real agent runs: a mock model drives `run_loop`
//! and the delegation host, so the run scope, tool classification, the revert
//! button, sandbox reports and the panel's Apply / Discard reviews are all
//! exercised through the runtime rather than by calling tools directly.

mod common;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_chat_core::{
    ChatProvider, ChatRequest, ChatResponse, ModelDescriptor, ToolCall, ToolDefinition,
};
use agent_chat_tools::ToolRegistry;
use agent_runtime::{
    pending_reviews, resolve_review, revert_checkpoint_tree, run_loop, CancelToken, ConfirmPolicy,
    DelegationHost, DelegationLimits, EntryKind, EventSink, HarnessRegistry, Ledger, LedgerEntry,
    ReviewDecision, RunConfig, RunEvent, RunStore, ToolClass, ToolClassifier,
};
use agent_scene_tools::{install_local, register_tools, FixedScene, SceneToolbox};
use serde_json::{json, Value};

/// The runtime keeps one process-wide extension registry; tests that install
/// their own toolbox take turns.
static EXTENSION_LOCK: Mutex<()> = Mutex::new(());

const REPORT: &str = "## Summary\nPlaced a lamp.\n## Findings\n- added one object\n## Refs\nnone\n## Open questions\nnone";

enum Step {
    Say(String),
    Call(&'static str, String, Value),
}

type Handler = Box<dyn Fn(&ChatRequest) -> Step + Send + Sync>;

struct Mock {
    handler: Handler,
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
        let (text, tool_calls) = match (self.handler)(&request) {
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

fn mock(handler: impl Fn(&ChatRequest) -> Step + Send + Sync + 'static) -> Arc<dyn ChatProvider> {
    Arc::new(Mock {
        handler: Box::new(handler),
    })
}

fn tool_results(req: &ChatRequest) -> Vec<String> {
    req.messages
        .iter()
        .filter(|m| m.role == agent_chat_core::ChatRole::Tool)
        .map(|m| m.content.clone())
        .collect()
}

fn is_child(req: &ChatRequest) -> bool {
    req.messages.first().is_some_and(|m| m.content.contains("scene editor"))
}

struct World {
    toolbox: Arc<SceneToolbox>,
    scene: agent_scene_tools::SharedScene,
    registry: ToolRegistry,
    root: tempfile::TempDir,
}

fn world() -> World {
    static ENGINE: std::sync::Once = std::sync::Once::new();
    ENGINE.call_once(|| engine_state::EngineContext::new().set_global());
    let fixed = FixedScene::new();
    let scene = fixed.handle();
    let toolbox = install_local(Arc::new(fixed));
    let mut registry = ToolRegistry::new();
    register_tools(&mut registry, &toolbox);
    World {
        toolbox,
        scene,
        registry,
        root: tempfile::tempdir().unwrap(),
    }
}

impl World {
    fn config(&self, provider: Arc<dyn ChatProvider>, run_id: &str) -> RunConfig {
        let mut config = RunConfig::new(
            provider,
            "main-model",
            self.registry.clone(),
            PathBuf::from(self.root.path()),
        );
        config.tools = self
            .registry
            .definitions()
            .into_iter()
            .map(|d| ToolDefinition {
                name: d.name,
                description: Some(d.description),
                parameters_json_schema: d.parameters_schema,
            })
            .collect();
        config.safety.run_id = run_id.to_string();
        config.safety.policy = ConfirmPolicy::Auto;
        config.max_iterations = 8;
        config
    }

    fn ids(&self) -> Vec<String> {
        agent_scene_tools::SceneImage::capture(&self.scene.read().world).order
    }
}

fn collecting_sink() -> (EventSink, Arc<Mutex<Vec<RunEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink: EventSink = {
        let events = events.clone();
        Arc::new(move |event| events.lock().unwrap().push(event))
    };
    (sink, events)
}

fn user_ledger(text: &str) -> Ledger {
    let mut ledger = Ledger::with_system_prompt("You are the main agent.");
    ledger.push(LedgerEntry::user(text));
    ledger
}

#[test]
fn the_extension_classifies_scene_tools() {
    let _guard = EXTENSION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let _world = world();
    let classifier = ToolClassifier::default();
    for read in agent_scene_tools::READ_TOOLS {
        assert_eq!(classifier.classify(read), ToolClass::Read, "{read}");
    }
    for write in agent_scene_tools::WRITE_TOOLS {
        assert_eq!(classifier.classify(write), ToolClass::Write, "{write}");
    }
    assert_eq!(classifier.classify("some_other_tool"), ToolClass::Write);
}

#[test]
fn a_run_edits_the_scene_and_its_revert_button_undoes_it() {
    let _guard = EXTENSION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let w = world();
    let provider = mock(|req| match tool_results(req).len() {
        0 => Step::Call(
            "scene_write_spawn_object",
            "c1".into(),
            json!({"name": "Lamp", "id": "lamp", "components": [{"class_name": "Lamp"}]}),
        ),
        1 => Step::Call(
            "scene_write_transform",
            "c2".into(),
            json!({"id": "lamp", "position": [0, 5, 0]}),
        ),
        _ => Step::Say("done".into()),
    });
    let config = w.config(provider, "run-e2e-1");
    let (sink, events) = collecting_sink();
    let ledger = run_loop(config, user_ledger("add a lamp"), CancelToken::new(), sink);

    // Both writes went through (no tool errors) and the scene changed.
    let results: Vec<_> = ledger
        .entries
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) => Some(x.result.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(|r| !r.is_error), "{results:?}");
    assert_eq!(w.ids(), vec!["lamp".to_string()]);
    let first: Value = serde_json::from_str(&results[0].content).unwrap();
    assert_eq!(first["transaction"]["author"], "agent:run-e2e-1");

    // The loop told the UI there is something to revert, once.
    let created = events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| matches!(e, RunEvent::CheckpointCreated { run_id } if run_id == "run-e2e-1"))
        .count();
    assert_eq!(created, 1);

    // Reverting the run through the same call the panel uses.
    assert!(agent_runtime::has_checkpoint_tree(w.root.path(), "run-e2e-1"));
    let report = revert_checkpoint_tree(w.root.path(), "run-e2e-1").unwrap();
    assert!(report.is_clean(), "{}", report.summary());
    assert!(report.summary().contains("Undid 2 scene transaction"), "{}", report.summary());
    assert!(w.ids().is_empty());
    assert!(!agent_runtime::has_checkpoint_tree(w.root.path(), "run-e2e-1"));
}

#[test]
fn a_read_only_run_cannot_write_the_scene() {
    let _guard = EXTENSION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let w = world();
    let provider = mock(|req| {
        if tool_results(req).is_empty() {
            Step::Call("scene_write_spawn_object", "c1".into(), json!({"name": "Nope"}))
        } else {
            Step::Say("ok".into())
        }
    });
    let mut config = w.config(provider, "run-e2e-ro");
    config.safety.read_only = true;
    let ledger = run_loop(config, user_ledger("try"), CancelToken::new(), collecting_sink().0);
    let result = ledger
        .entries
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::ToolExchange(x) => Some(x.result.clone()),
            _ => None,
        })
        .unwrap();
    assert!(result.is_error && result.content.contains("read-only"), "{}", result.content);
    assert!(w.ids().is_empty());
}

#[test]
fn a_sandbox_child_returns_a_diff_and_the_user_applies_it() {
    let _guard = EXTENSION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let w = world();
    // The live scene has one object the child will rename.
    {
        let ctx = common::scope("seed", false);
        w.registry
            .execute("scene_write_spawn_object", json!({"name": "Old", "id": "old"}), &ctx)
            .unwrap();
    }
    let baseline = w.ids();

    let provider = mock(|req| {
        if is_child(req) {
            return match tool_results(req).len() {
                0 => Step::Call(
                    "scene_write_spawn_object",
                    "k1".into(),
                    json!({"name": "Lamp", "id": "lamp", "position": [0, 3, 0]}),
                ),
                1 => Step::Call(
                    "scene_write_set_object",
                    "k2".into(),
                    json!({"id": "old", "name": "Renamed"}),
                ),
                _ => Step::Say(REPORT.into()),
            };
        }
        let results = tool_results(req);
        if results.is_empty() {
            Step::Call(
                "spawn_subagent",
                "p1".into(),
                json!({"harness": "scene-sandbox", "task": "place a lamp and rename old"}),
            )
        } else {
            Step::Say("the sandbox finished".into())
        }
    });
    let store = RunStore::in_memory();
    let (events, host_events) = collecting_sink();
    let host = DelegationHost::new(store.clone(), HarnessRegistry::builtin(), DelegationLimits::default(), events);
    let mut config = w.config(provider, "run-e2e-sbx");
    config.delegation = Some(host.root_handle(config.clone()));

    let ledger = run_loop(config, user_ledger("do it in a sandbox"), CancelToken::new(), collecting_sink().0);

    // The live scene is unchanged: the child worked in its fork.
    assert_eq!(w.ids(), baseline);

    // The parent got the child's report with the diff attached.
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
    assert_eq!(report["summary"], "Placed a lamp.");
    let diff = report["extra"]["scene_diff"]
        .as_str()
        .expect("the diff section")
        .to_string();
    assert!(diff.contains("+1 added, -0 removed, ~1 changed"), "{diff}");
    assert!(diff.contains("+ lamp \"Lamp\""), "{diff}");
    assert!(diff.contains("name \"Old\" -> \"Renamed\""), "{diff}");

    // The panel's review list has it; Apply replays it as one transaction.
    let reviews = pending_reviews();
    assert_eq!(reviews.len(), 1, "{reviews:?}");
    assert!(reviews[0].title.contains("+1 added"));
    assert!(reviews[0].id.starts_with("run-e2e-sbx."), "{}", reviews[0].id);
    let message = resolve_review(&reviews[0].id, ReviewDecision::Apply).unwrap();
    assert!(message.contains("one transaction"), "{message}");
    assert!(w.ids().contains(&"lamp".to_string()));
    assert!(pending_reviews().is_empty());

    // The applied diff belongs to the run tree, so the revert button undoes it.
    assert!(agent_runtime::has_checkpoint_tree(w.root.path(), "run-e2e-sbx"));
    revert_checkpoint_tree(w.root.path(), "run-e2e-sbx").unwrap();
    assert_eq!(w.ids(), baseline);
    let old = w
        .toolbox
        .read(&Default::default(), |world| {
            let index = agent_scene_tools::snapshot::SceneIndex::build(world);
            let e = index.entity("old").unwrap();
            Ok(agent_scene_tools::ops::object_summary(world, &index, e)["name"].clone())
        })
        .unwrap();
    assert_eq!(old, "Old");

    // The card events fired.
    assert!(host_events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, RunEvent::ChildFinished { .. })));
}

#[test]
fn discarding_a_sandbox_review_leaves_the_scene_alone() {
    let _guard = EXTENSION_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let w = world();
    let provider = mock(|req| {
        if is_child(req) {
            return if tool_results(req).is_empty() {
                Step::Call("scene_write_spawn_object", "k1".into(), json!({"name": "Temp", "id": "temp"}))
            } else {
                Step::Say(REPORT.into())
            };
        }
        if tool_results(req).is_empty() {
            Step::Call(
                "spawn_subagent",
                "p1".into(),
                json!({"harness": "scene-sandbox", "task": "add temp"}),
            )
        } else {
            Step::Say("ok".into())
        }
    });
    let store = RunStore::in_memory();
    let host = DelegationHost::new(
        store,
        HarnessRegistry::builtin(),
        DelegationLimits::default(),
        collecting_sink().0,
    );
    let mut config = w.config(provider, "run-e2e-discard");
    config.delegation = Some(host.root_handle(config.clone()));
    run_loop(config, user_ledger("go"), CancelToken::new(), collecting_sink().0);

    let reviews = pending_reviews();
    assert_eq!(reviews.len(), 1);
    resolve_review(&reviews[0].id, ReviewDecision::Discard).unwrap();
    assert!(pending_reviews().is_empty());
    assert!(w.ids().is_empty());
    assert!(resolve_review(&reviews[0].id, ReviewDecision::Apply).is_err());
}
