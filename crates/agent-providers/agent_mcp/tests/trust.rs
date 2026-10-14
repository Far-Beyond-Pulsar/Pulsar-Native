//! A project's `.pulsar/mcp.json` launches processes, so its servers need the
//! user's approval before the first launch.

use std::{sync::Arc, time::Duration};

use agent_mcp::{McpManager, ServerStatus};
use agent_runtime::trust::{PendingKind, TrustState, TrustStore};

const FAKE: &str = env!("CARGO_BIN_EXE_fake_mcp_server");

fn write_config(root: &std::path::Path, args: &str) {
    std::fs::create_dir_all(root.join(".pulsar")).unwrap();
    let command = FAKE.replace('\\', "\\\\");
    std::fs::write(
        root.join(".pulsar/mcp.json"),
        format!(r#"{{"mcpServers":{{"proj":{{"command":"{command}","args":["{args}"]}}}}}}"#),
    )
    .unwrap();
}

#[test]
fn project_servers_do_not_launch_until_approved() {
    let dir = tempfile::tempdir().unwrap();
    write_config(dir.path(), "ok");
    let trust = Arc::new(TrustStore::in_memory());

    let manager = McpManager::load(dir.path(), trust.clone());
    manager.start_all();
    assert!(manager.wait_ready(Duration::from_millis(200)));
    let snapshot = manager.snapshot();
    assert_eq!(snapshot[0].status, ServerStatus::AwaitingApproval);
    assert!(snapshot[0].tools.is_empty());
    // A restart request does not bypass the gate either.
    manager.restart_server("proj");
    assert_eq!(manager.snapshot()[0].status, ServerStatus::AwaitingApproval);

    // The user is shown the exact command and arguments.
    let pending = manager.pending_approvals();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].kind, PendingKind::Mcp);
    assert_eq!(pending[0].state, TrustState::New);
    assert!(pending[0].content.contains("command: "));
    assert!(pending[0].content.contains("args: ok"));

    manager.approve("proj");
    assert!(manager.wait_ready(Duration::from_secs(10)));
    assert_eq!(manager.snapshot()[0].status, ServerStatus::Running);
    assert!(manager.pending_approvals().is_empty());
    manager.shutdown();

    // Next session: the approval is remembered.
    let again = McpManager::load(dir.path(), trust.clone());
    assert_eq!(again.snapshot()[0].status, ServerStatus::Pending);
    again.shutdown();

    // The repository changes the command line: approval is needed again.
    write_config(dir.path(), "crash");
    let changed = McpManager::load(dir.path(), trust);
    assert_eq!(changed.snapshot()[0].status, ServerStatus::AwaitingApproval);
    let pending = changed.pending_approvals();
    assert_eq!(pending[0].state, TrustState::Changed);
    assert!(pending[0].diff.as_deref().unwrap().contains("crash"));
}
