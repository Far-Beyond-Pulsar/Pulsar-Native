//! Phase 6 integration: features built on separate branches that must still
//! agree when they meet in one run.
//!
//! - plan mode vs. MCP and scene (run-extension) write tools
//! - cascade escalation vs. pinned requirements
//! - vision images vs. non-vision child models
//! - a run extension's tools vs. the read-only harness trim

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_chat_core::{ChatRole, ToolDefinition};
use agent_chat_tools::{visual::CaptureStore, ToolRegistry};
use agent_runtime::{
    run_loop,
    testing::{MockProvider, ScriptedStep},
    CancelToken, CascadeSettings, ConfirmPolicy, DelegationHost, DelegationLimits, EventSink,
    Harness, HarnessOrigin, HarnessRegistry, Ledger, LedgerEntry, ModelProfile, PinAuthor,
    PinKind, PlanMode, RunConfig, RunEvent, RunExtension, RunStore, ToolClass, EXIT_PLAN_MODE,
};
use serde_json::json;

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

fn definition(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: Some(name.to_string()),
        parameters_json_schema: json!({"type": "object", "properties": {}}),
    }
}

/// Stands in for the scene tools: a registered extension that owns one read
/// and one write tool, the way `agent_scene_tools` does.
struct FakeSceneExtension;

impl RunExtension for FakeSceneExtension {
    fn name(&self) -> &'static str {
        "cross-feature-fake-scene"
    }
    fn classify(&self, tool: &str) -> Option<ToolClass> {
        match tool {
            "xscene_query" => Some(ToolClass::Read),
            "xscene_write_spawn" => Some(ToolClass::Write),
            _ => None,
        }
    }
}

fn ok_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_fn("ok", "Works", json!({"type": "object"}), |_, _| Ok(json!({"fine": true})));
    registry
}

fn ran_registry(names: &[&str]) -> (ToolRegistry, Arc<Mutex<Vec<String>>>) {
    let ran = Arc::new(Mutex::new(Vec::new()));
    let mut registry = ToolRegistry::new();
    for name in names {
        let ran = ran.clone();
        let name = name.to_string();
        registry.register_fn(
            &name.clone(),
            &name.clone(),
            json!({"type": "object", "properties": {}}),
            move |_args, _ctx| {
                ran.lock().unwrap().push(name.clone());
                Ok(json!({"ok": true}))
            },
        );
    }
    (registry, ran)
}

// ---------------------------------------------------------------------------

#[test]
fn plan_mode_never_reaches_mcp_or_scene_write_tools() {
    init_engine();
    agent_runtime::register_run_extension(Arc::new(FakeSceneExtension));
    let all = [
        "read_file",
        "mcp__srv__edit",
        "mcp__srv__peek",
        "xscene_query",
        "xscene_write_spawn",
    ];
    let (registry, ran) = ran_registry(&all);
    let provider = MockProvider::from_fn(|request, index| {
        let plan = json!({"goal": "g", "steps": [{"title": "s"}]});
        Ok(match index {
            // The model ignores plan mode and tries everything.
            0 => {
                let mut step = ScriptedStep::say("");
                for (i, name) in [
                    "read_file",
                    "mcp__srv__edit",
                    "mcp__srv__peek",
                    "xscene_query",
                    "xscene_write_spawn",
                ]
                .iter()
                .enumerate()
                {
                    step.tool_calls
                        .extend(ScriptedStep::call(name, &format!("c{i}"), json!({})).tool_calls);
                }
                // The request itself must already lack the write tools.
                let offered: Vec<&str> = request.tools.iter().map(|t| t.name.as_str()).collect();
                assert!(!offered.contains(&"mcp__srv__edit"), "{offered:?}");
                assert!(!offered.contains(&"xscene_write_spawn"), "{offered:?}");
                assert!(offered.contains(&"read_file") && offered.contains(&EXIT_PLAN_MODE));
                step.into_response()
            }
            1 => ScriptedStep::call(EXIT_PLAN_MODE, "p", plan).into_response(),
            _ => panic!("the run ends once the plan is submitted"),
        })
    });
    let mut config = RunConfig::new(provider, "model", registry, PathBuf::from("."));
    config.tools = all.iter().map(|n| definition(n)).collect();
    config.safety.policy = ConfirmPolicy::Auto;
    config.safety.checkpoints = false;
    // The user marked `peek` read-only in the MCP config.
    config.safety.classifier.set("mcp__srv__peek", ToolClass::Read);
    let mode = PlanMode::new();
    config.plan_mode = Some(mode.clone());

    let mut ledger = Ledger::with_system_prompt("base");
    ledger.push(LedgerEntry::user("plan a health bar"));
    let (sink, _events) = sink();
    run_loop(config, ledger, CancelToken::new(), sink);

    let ran = ran.lock().unwrap().clone();
    assert!(ran.contains(&"read_file".to_string()));
    assert!(ran.contains(&"xscene_query".to_string()));
    assert!(ran.contains(&"mcp__srv__peek".to_string()));
    assert!(!ran.contains(&"mcp__srv__edit".to_string()), "ran: {ran:?}");
    assert!(!ran.contains(&"xscene_write_spawn".to_string()), "ran: {ran:?}");
    assert!(mode.take().is_some(), "the plan was submitted");
}

#[test]
fn a_read_only_harness_trims_extension_and_mcp_write_tools() {
    agent_runtime::register_run_extension(Arc::new(FakeSceneExtension));
    let mut classifier = agent_runtime::ToolClassifier::new();
    classifier.set("mcp__srv__peek", ToolClass::Read);
    let available: Vec<String> = [
        "read_file",
        "mcp__srv__edit",
        "mcp__srv__peek",
        "xscene_query",
        "xscene_write_spawn",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut harness = Harness::parse(
        "---\nname: scout\ndescription: d\ntools: [\"*\"]\n---\nbody",
        "scout",
        HarnessOrigin::Project,
    )
    .unwrap();
    harness.tools = vec!["*".to_string()];
    let allowed = harness.resolve_tools_with(
        &available,
        &classifier,
        agent_runtime::HarnessPermissions::ReadOnly,
    );
    assert!(allowed.contains(&"xscene_query".to_string()), "{allowed:?}");
    assert!(allowed.contains(&"mcp__srv__peek".to_string()), "{allowed:?}");
    assert!(!allowed.contains(&"mcp__srv__edit".to_string()), "{allowed:?}");
    assert!(!allowed.contains(&"xscene_write_spawn".to_string()), "{allowed:?}");
}

#[test]
fn cascade_escalation_keeps_pinned_requirements_in_every_request() {
    init_engine();
    let provider = MockProvider::from_fn(|request, _| {
        Ok(if request.model == "big" {
            ScriptedStep::say("Handled by the big model.").into_response()
        } else {
            // Not valid tool-call protocol: counts as malformed.
            ScriptedStep::say("```tool_call\nnot json\n```").into_response()
        })
    });
    let mut profile = ModelProfile::new(8_192, false);
    profile.parallel_tools = false;
    let mut config = RunConfig::new(provider.clone(), "small", ok_registry(), PathBuf::from("."))
        .with_profile(profile);
    config.tools = vec![definition("ok")];
    config.cascade = CascadeSettings {
        enabled: true,
        escalation_model: Some("big".into()),
        ..CascadeSettings::default()
    }
    .policy();
    assert!(config.cascade.is_some());

    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.add_pin(PinKind::Requirement, "Never touch the vendored submodules.", PinAuthor::User);
    ledger.push(LedgerEntry::user("go"));
    let (sink, events) = sink();
    let ledger = run_loop(config, ledger, CancelToken::new(), sink);

    let requests = provider.requests();
    assert!(requests.iter().any(|r| r.model == "big"), "no escalation happened");
    for request in &requests {
        let system = &request.messages[0];
        assert_eq!(system.role, ChatRole::System);
        assert!(
            system.content.contains("- Never touch the vendored submodules."),
            "model {} lost the pin",
            request.model
        );
    }
    assert!(events.lock().unwrap().iter().any(|e| matches!(e, RunEvent::Escalated(_))));
    assert_eq!(ledger.pins().len(), 1, "escalation does not edit the pin ledger");
}

#[test]
fn child_runs_on_text_only_models_never_receive_images() {
    init_engine();
    let dir = tempfile::tempdir().unwrap();
    let store = CaptureStore::new(dir.path());
    let (capture_id, path) = store.allocate().unwrap();
    std::fs::write(path, b"PNGDATA").unwrap();

    let result = json!({"ok": true, "captures": [{"ref": capture_id}]});
    let mut registry = ToolRegistry::new();
    registry.register_fn("snap", "Take a capture", json!({"type": "object"}), move |_a, _c| {
        Ok(result.clone())
    });

    let is_child = |request: &agent_chat_core::ChatRequest| {
        request.messages.first().is_some_and(|m| m.content.contains("CHILD"))
    };
    let provider = MockProvider::from_fn(move |request, _| {
        let tool_results = request.messages.iter().filter(|m| m.role == ChatRole::Tool).count();
        Ok(if is_child(request) {
            match tool_results {
                0 => ScriptedStep::call("snap", "k1", json!({})),
                _ => ScriptedStep::say(
                    "{\"status\":\"success\",\"summary\":\"saw it\",\"findings\":[],\"refs\":[],\"open_questions\":[]}",
                ),
            }
        } else {
            match tool_results {
                0 => ScriptedStep::call(
                    "spawn_subagent",
                    "p1",
                    json!({"harness": "viewer", "task": "look"}),
                ),
                1 => ScriptedStep::call("snap", "p2", json!({})),
                _ => ScriptedStep::say("all done"),
            }
        }
        .into_response())
    });

    let mut harnesses = HarnessRegistry::builtin();
    harnesses.insert(
        Harness::parse(
            "---\nname: viewer\ndescription: d\ntools: [snap]\nmodel: text-child\nmax_steps: 6\n---\nCHILD viewer.\n{pinned}",
            "viewer",
            HarnessOrigin::Project,
        )
        .unwrap(),
    );
    let (host_sink, _) = sink();
    let host = DelegationHost::new(RunStore::in_memory(), harnesses, DelegationLimits::default(), host_sink);
    host.set_profile_resolver(Some(Arc::new(|_model: &str| {
        let mut profile = ModelProfile::new(32_768, true);
        profile.vision = false;
        profile
    })));

    let mut parent_profile = ModelProfile::new(32_768, true);
    parent_profile.vision = true;
    let mut template =
        RunConfig::new(provider.clone(), "vision-parent", registry, dir.path().to_path_buf())
            .with_profile(parent_profile);
    template.tools = vec![definition("snap")];
    template.safety.classifier.set("snap", ToolClass::Read);
    template.safety.policy = ConfirmPolicy::Auto;
    template.safety.checkpoints = false;
    let mut config = template.clone();
    config.delegation = Some(host.root_handle(template));

    let mut ledger = Ledger::with_system_prompt("PARENT");
    ledger.push(LedgerEntry::user("delegate"));
    let (sink, _) = sink();
    run_loop(config, ledger, CancelToken::new(), sink);

    let requests = provider.requests();
    let child_with_result: Vec<_> = requests
        .iter()
        .filter(|r| is_child(r) && r.messages.iter().any(|m| m.role == ChatRole::Tool))
        .collect();
    assert!(!child_with_result.is_empty(), "the child never saw its capture result");
    for request in &child_with_result {
        for message in request.messages.iter().filter(|m| m.role == ChatRole::Tool) {
            assert!(message.images.is_empty(), "text-only child received an image");
            assert!(message.content.contains("cannot see images"));
        }
    }
    let parent_got_image = requests
        .iter()
        .filter(|r| !is_child(r))
        .flat_map(|r| r.messages.iter())
        .any(|m| m.role == ChatRole::Tool && !m.images.is_empty());
    assert!(parent_got_image, "the vision parent should receive the capture");
}

#[test]
fn run_extension_classification_is_what_the_single_classifier_reports() {
    agent_runtime::register_run_extension(Arc::new(FakeSceneExtension));
    let classifier = agent_runtime::ToolClassifier::new();
    assert_eq!(classifier.classify("xscene_query"), ToolClass::Read);
    assert_eq!(classifier.classify("xscene_write_spawn"), ToolClass::Write);
    assert_eq!(classifier.classify("capture_viewport"), ToolClass::Read);
    assert_eq!(classifier.classify("run_and_observe"), ToolClass::Write);
    assert_eq!(classifier.classify("mcp__any__tool"), ToolClass::Write);
}
