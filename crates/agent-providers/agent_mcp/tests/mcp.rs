//! Client, manager, server and safety classification of MCP tools, against a
//! hand-written fake stdio server (`fake_mcp_server`) and the real
//! `pulsar_mcp_server` binary.

use std::{
    collections::BTreeMap,
    io::Cursor,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use agent_chat_tools::{ToolContext, ToolRegistry};
use agent_mcp::{
    serve, McpClient, McpConfig, McpError, McpManager, ServerConfig, ServerOptions, ServerStatus,
};
use agent_runtime::{
    is_mcp_tool, router::Category, ConfirmPolicy, ToolClass, ToolClassifier,
};
use serde_json::{json, Value};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_mcp_server");

fn fake_config(mode: &str, timeout_secs: u64) -> ServerConfig {
    ServerConfig {
        command: Some(FAKE.into()),
        args: vec![mode.into()],
        timeout_secs,
        read_only_tools: vec!["read_thing".into()],
        ..ServerConfig::default()
    }
}

fn manager_with(servers: &[(&str, ServerConfig)], root: &Path) -> Arc<McpManager> {
    let mut cfg = McpConfig::default();
    for (name, c) in servers {
        cfg.servers.insert(name.to_string(), c.clone());
    }
    McpManager::with_config(root, cfg)
}

fn ctx() -> ToolContext {
    ToolContext::default()
}

#[test]
fn client_lists_and_calls_tools() {
    let dir = tempfile::tempdir().unwrap();
    let client = McpClient::spawn("fake", &fake_config("ok", 10), dir.path()).unwrap();
    let tools = client.list_tools().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["echo", "read_thing", "fail"]);
    let out = client.call_tool("echo", json!({"text": "hi"})).unwrap();
    assert_eq!(out["content"][0]["text"], "hi");
}

#[test]
fn missing_binary_is_a_spawn_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ServerConfig {
        command: Some("definitely-not-a-real-mcp-binary".into()),
        ..ServerConfig::default()
    };
    assert!(matches!(
        McpClient::spawn("x", &cfg, dir.path()),
        Err(McpError::Spawn(_))
    ));
    let http = ServerConfig {
        url: Some("http://localhost:1/mcp".into()),
        ..ServerConfig::default()
    };
    assert!(McpClient::spawn("x", &http, dir.path()).is_err());
}

#[test]
fn server_dying_during_initialize_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let err = McpClient::spawn("fake", &fake_config("die-on-init", 10), dir.path()).err();
    assert!(matches!(err, Some(McpError::Disconnected(_))), "{err:?}");
}

#[test]
fn hanging_call_times_out_and_the_client_stays_usable() {
    let dir = tempfile::tempdir().unwrap();
    let client = McpClient::spawn("fake", &fake_config("hang-on-call", 1), dir.path()).unwrap();
    let started = Instant::now();
    let err = client.call_tool("echo", json!({"text": "x"})).unwrap_err();
    assert!(matches!(err, McpError::Timeout(_)), "{err}");
    assert!(started.elapsed() < Duration::from_secs(5));
    // Other requests still work after a timeout.
    assert_eq!(client.list_tools().unwrap().len(), 3);
}

#[test]
fn manager_registers_namespaced_tools_and_isolates_a_crash() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager_with(&[("fake", fake_config("crash-on-call", 5))], dir.path());
    mgr.start_all();
    assert!(mgr.wait_ready(Duration::from_secs(10)));

    let mut registry = ToolRegistry::new();
    mgr.register_into(&mut registry);
    assert_eq!(
        registry.names(),
        ["mcp__fake__echo", "mcp__fake__fail", "mcp__fake__read_thing"]
    );
    let defs = registry.definitions();
    assert!(defs.iter().all(|d| d.category.as_deref() == Some("mcp")));
    assert!(defs[0].description.starts_with("[MCP server 'fake']"));

    // The server dies mid-call: an Err for this call only, no panic.
    let err = registry
        .execute("mcp__fake__echo", json!({"text": "x"}), &ctx())
        .unwrap_err();
    assert!(err.to_string().contains("disconnected") || err.to_string().contains("exited"), "{err}");

    // Status flips to crashed and later calls fail fast with a clear message.
    let snap = mgr.snapshot();
    assert!(matches!(snap[0].status, ServerStatus::Crashed(_)), "{:?}", snap[0].status);
    let err = registry
        .execute("mcp__fake__echo", json!({"text": "x"}), &ctx())
        .unwrap_err();
    assert!(err.to_string().contains("not available"), "{err}");

    // A refreshed registry drops the dead server's tools.
    mgr.register_into(&mut registry);
    assert!(registry.names().is_empty());

    // Restart brings it back.
    mgr.restart_server("fake");
    assert!(mgr.wait_ready(Duration::from_secs(10)));
    mgr.register_into(&mut registry);
    assert_eq!(registry.names().len(), 3);
}

#[test]
fn tool_results_and_errors_reach_the_model() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager_with(&[("fake", fake_config("ok", 5))], dir.path());
    mgr.start_all();
    assert!(mgr.wait_ready(Duration::from_secs(10)));
    let mut registry = ToolRegistry::new();
    mgr.register_into(&mut registry);

    let ok = registry
        .execute("mcp__fake__echo", json!({"text": "hello"}), &ctx())
        .unwrap();
    assert_eq!(ok["content"], "hello");
    let err = registry
        .execute("mcp__fake__fail", json!({}), &ctx())
        .unwrap_err();
    assert!(err.to_string().contains("it failed"));
    // A tool-level error does not take the server down.
    assert_eq!(mgr.snapshot()[0].status, ServerStatus::Running);
}

#[test]
fn failed_and_disabled_servers_are_listed_and_do_not_break_others() {
    let dir = tempfile::tempdir().unwrap();
    let bad = ServerConfig {
        command: Some("definitely-not-a-real-mcp-binary".into()),
        ..ServerConfig::default()
    };
    let off = ServerConfig {
        enabled: false,
        ..fake_config("ok", 5)
    };
    let mgr = manager_with(
        &[("bad", bad), ("good", fake_config("ok", 5)), ("off", off)],
        dir.path(),
    );
    mgr.start_all();
    assert!(mgr.wait_ready(Duration::from_secs(10)));
    let status: BTreeMap<String, ServerStatus> = mgr
        .snapshot()
        .into_iter()
        .map(|s| (s.name, s.status))
        .collect();
    assert!(matches!(status["bad"], ServerStatus::Failed(_)));
    assert_eq!(status["good"], ServerStatus::Running);
    assert_eq!(status["off"], ServerStatus::Disabled);
    let mut registry = ToolRegistry::new();
    mgr.register_into(&mut registry);
    assert!(registry.names().iter().all(|n| n.starts_with("mcp__good__")));
}

#[test]
fn disabled_tools_are_not_registered_and_the_choice_persists() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager_with(&[("fake", fake_config("ok", 5))], dir.path());
    mgr.start_all();
    assert!(mgr.wait_ready(Duration::from_secs(10)));
    mgr.set_tool_enabled("mcp__fake__fail", false);

    let mut registry = ToolRegistry::new();
    mgr.register_into(&mut registry);
    assert!(!registry.contains("mcp__fake__fail"));
    let snap = mgr.snapshot();
    assert!(!snap[0].tools.iter().find(|t| t.name == "fail").unwrap().enabled);

    // A fresh manager (next editor start) remembers.
    let again = manager_with(&[("fake", fake_config("ok", 5))], dir.path());
    assert!(!again.is_tool_enabled("mcp__fake__fail"));
    assert!(again.is_tool_enabled("mcp__fake__echo"));
}

#[test]
fn shutdown_kills_the_server_process() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager_with(&[("fake", fake_config("ok", 5))], dir.path());
    mgr.start_all();
    assert!(mgr.wait_ready(Duration::from_secs(10)));
    let mut registry = ToolRegistry::new();
    mgr.register_into(&mut registry);
    mgr.shutdown();
    let err = registry
        .execute("mcp__fake__echo", json!({"text": "x"}), &ctx())
        .unwrap_err();
    assert!(err.to_string().contains("not available"), "{err}");
}

// ---- classification and taint ------------------------------------------

#[test]
fn mcp_tools_are_write_class_unless_vouched_read_only_and_always_taint() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = manager_with(&[("fake", fake_config("ok", 5))], dir.path());
    mgr.start_all();
    assert!(mgr.wait_ready(Duration::from_secs(10)));

    let mut classifier = ToolClassifier::new();
    assert_eq!(classifier.classify("mcp__fake__echo"), ToolClass::Write);
    mgr.apply_classification(&mut classifier);
    assert_eq!(classifier.classify("mcp__fake__echo"), ToolClass::Write);
    assert_eq!(classifier.classify("mcp__fake__read_thing"), ToolClass::Read);

    // Read-only or not, the result is untrusted content.
    let root = Path::new(".");
    for tool in ["mcp__fake__echo", "mcp__fake__read_thing"] {
        assert!(classifier.taints_on_result(tool, &json!({}), root), "{tool}");
    }
    assert!(!classifier.taints_on_result("list_files", &json!({}), root));
    assert!(is_mcp_tool("mcp__a__b") && !is_mcp_tool("read_file"));
    // The default policy asks for write-class calls once tainted.
    assert_eq!(ConfirmPolicy::default(), ConfirmPolicy::AskWhenTainted);
}

#[test]
fn router_hides_mcp_tools_until_more_tools_unlocks_them() {
    use agent_chat_core::ToolDefinition;
    use agent_runtime::router::{RouteContext, ToolRouter};

    let def = |name: &str, desc: &str| ToolDefinition {
        name: name.into(),
        description: Some(desc.into()),
        parameters_json_schema: json!({"type": "object", "properties": {}}),
    };
    let router = ToolRouter::new(&[
        def("read_file", "Read a file"),
        def("mcp__fake__echo", "Echo text back"),
    ]);
    assert_eq!(
        agent_runtime::router::categorize("mcp__fake__echo", "").name(),
        "mcp"
    );
    assert!(router.hides_tools(8), "mcp tools make more_tools available");
    let base = RouteContext {
        last_user_message: "echo some text back with the fake server",
        ..RouteContext::default()
    };
    let picked = router.select(&base, 8);
    assert!(picked.iter().all(|d| d.name != "mcp__fake__echo"));
    let unlocked = [Category::Mcp];
    let picked = router.select(&RouteContext { unlocked: &unlocked, ..base }, 8);
    assert!(picked.iter().any(|d| d.name == "mcp__fake__echo"));
    let (cat, _) = router.unlock("mcp").unwrap();
    assert_eq!(cat, Category::Mcp);
}

// ---- server side --------------------------------------------------------

fn demo_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    r.register_fn("read_file", "Read", json!({"type":"object","properties":{}}), |_, _| {
        Ok(json!({"ok": true, "text": "contents"}))
    });
    r.register_fn("write_file", "Write", json!({"type":"object","properties":{}}), |_, _| {
        Ok(json!({"ok": true}))
    });
    r.register_fn("boom", "Panics", json!({"type":"object","properties":{}}), |_, _| {
        panic!("tool bug")
    });
    r.register_fn("mcp__x__y", "Proxy", json!({"type":"object","properties":{}}), |_, _| {
        Ok(json!({}))
    });
    r
}

fn run_server(opts: &ServerOptions, requests: &[Value]) -> Vec<Value> {
    let input: String = requests.iter().map(|r| format!("{r}\n")).collect();
    let mut out = Vec::new();
    serve(
        &demo_registry(),
        &ctx(),
        &ToolClassifier::new(),
        opts,
        Cursor::new(input),
        &mut out,
    )
    .unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn rpc(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}

#[test]
fn server_exposes_the_read_only_subset_by_default() {
    let replies = run_server(
        &ServerOptions::default(),
        &[
            rpc(1, "initialize", json!({"protocolVersion": "2025-03-26"})),
            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            rpc(2, "tools/list", json!({})),
            rpc(3, "tools/call", json!({"name":"read_file","arguments":{}})),
            rpc(4, "tools/call", json!({"name":"write_file","arguments":{}})),
            rpc(5, "tools/call", json!({"name":"mcp__x__y","arguments":{}})),
            rpc(6, "nope", json!({})),
        ],
    );
    assert_eq!(replies.len(), 6, "notification gets no reply");
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["read_file"]);
    assert_eq!(replies[2]["result"]["isError"], false);
    assert_eq!(replies[2]["result"]["structuredContent"]["text"], "contents");
    assert_eq!(replies[3]["error"]["code"], -32602);
    assert_eq!(replies[4]["error"]["code"], -32602);
    assert_eq!(replies[5]["error"]["code"], -32601);
}

#[test]
fn server_write_tools_are_opt_in_and_tool_panics_are_contained() {
    let mut opts = ServerOptions::default();
    opts.allow_write = true;
    let replies = run_server(
        &opts,
        &[
            rpc(1, "tools/call", json!({"name":"write_file","arguments":{}})),
            rpc(2, "tools/call", json!({"name":"boom","arguments":{}})),
            rpc(3, "ping", json!({})),
        ],
    );
    assert_eq!(replies[0]["result"]["isError"], false);
    assert_eq!(replies[1]["result"]["isError"], true);
    assert_eq!(replies[2]["result"], json!({}));

    let mut extra = ServerOptions::default();
    extra.extra_allowed.insert("write_file".into());
    let replies = run_server(&extra, &[rpc(1, "tools/list", json!({}))]);
    assert_eq!(replies[0]["result"]["tools"].as_array().unwrap().len(), 2);
}

#[test]
fn client_can_drive_the_real_engine_server_binary() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = ServerConfig {
        command: Some(env!("CARGO_BIN_EXE_pulsar_mcp_server").into()),
        args: vec!["--workspace".into(), dir.path().display().to_string()],
        timeout_secs: 60,
        ..ServerConfig::default()
    };
    let client = McpClient::spawn("pulsar", &cfg, dir.path()).unwrap();
    let tools = client.list_tools().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"task_list_read"), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("mcp__")),
        "proxy tools are never exposed"
    );
}
