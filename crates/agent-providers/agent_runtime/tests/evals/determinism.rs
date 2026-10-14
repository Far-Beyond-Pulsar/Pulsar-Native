//! Projection is a pure function of the ledger: the same ledger gives
//! byte-identical messages, however it got there.

use agent_chat_core::ToolCall;
use agent_runtime::{
    project, Ledger, LedgerEntry, ModelProfile, PinAuthor, PinKind, SubagentReport,
    SubagentStatus, ToolCalling, ToolResult,
};
use serde_json::json;

use crate::common::{assert_golden, render};

fn busy_ledger() -> Ledger {
    let mut ledger = Ledger::with_system_prompt("You are the Pulsar assistant.");
    ledger.add_pin(PinKind::Requirement, "Never edit vendored code.", PinAuthor::User);
    ledger.add_pin(PinKind::Decision, "Use SceneDB for scene state.", PinAuthor::User);
    ledger.push(LedgerEntry::user("Add a camera."));
    ledger.push(LedgerEntry::assistant("Looking."));
    ledger.push(LedgerEntry::tool_exchange(
        ToolCall {
            id: "c1".to_string(),
            name: "read_file".to_string(),
            arguments_json: json!({"path": "src/camera.rs"}),
        },
        ToolResult {
            content: "pub struct Camera;".to_string(),
            is_error: false,
        },
    ));
    ledger.push(LedgerEntry::subagent_report(SubagentReport {
        run_id: "run-1".to_string(),
        status: SubagentStatus::Success,
        summary: "Camera is in src/camera.rs".to_string(),
        refs: vec!["src/camera.rs:1".to_string()],
    }));
    ledger.push(LedgerEntry::engine_notice("Context was trimmed."));
    ledger.push(LedgerEntry::assistant("Done."));
    ledger
}

#[test]
fn same_ledger_projects_to_byte_identical_messages() {
    for mode in [ToolCalling::Native, ToolCalling::Weak, ToolCalling::None] {
        let mut profile = ModelProfile::new(8_192, true);
        profile.tool_calling = mode;
        let ledger = busy_ledger();

        let first = render(&project(&ledger, &profile));
        // Repeated, cloned and persisted-then-restored ledgers all agree.
        assert_eq!(first, render(&project(&ledger, &profile)), "{mode:?} repeat");
        assert_eq!(first, render(&project(&ledger.clone(), &profile)), "{mode:?} clone");
    }
}

/// Tool exchanges are memory-only today (see `LedgerEntry::to_persisted`), so
/// the persistence round trip is checked on a ledger without them.
#[test]
fn a_persisted_and_restored_ledger_projects_identically() {
    let mut ledger = Ledger::with_system_prompt("You are the Pulsar assistant.");
    ledger.add_pin(PinKind::Requirement, "Never edit vendored code.", PinAuthor::User);
    ledger.push(LedgerEntry::user("Add a camera."));
    ledger.push(LedgerEntry::assistant("Done."));
    ledger.push(LedgerEntry::engine_notice("Context was trimmed."));
    let profile = ModelProfile::new(8_192, true);
    let restored = Ledger::from_persisted(ledger.to_persisted());
    assert_eq!(render(&project(&ledger, &profile)), render(&project(&restored, &profile)));
}

#[test]
fn golden_busy_projection() {
    let profile = ModelProfile::new(8_192, true);
    assert_golden("busy_native", &render(&project(&busy_ledger(), &profile)));
}
