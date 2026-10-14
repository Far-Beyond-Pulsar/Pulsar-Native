use serde::Deserialize;

use crate::ledger::{EntryKind, Ledger, LedgerEntry, PinKind, SubagentReport, SubagentStatus};
use crate::persist::PersistedEntry;
use crate::profile::ModelProfile;
use crate::project::project;

/// A chat file as the panel wrote it before `ChatRole::AgentEvent` was removed.
const LEGACY_CHAT: &str = r#"{
    "id": "chat-1",
    "messages": [
        {"role": "system", "content": "You are an AI assistant."},
        {"role": "user", "content": "find the parser"},
        {"role": "assistant", "content": "spawning a helper"},
        {"role": "agent_event", "content": "Sub-agent 'Searcher' (id=subagent-7) completed — status: success.\n\nFindings preview:\nThe parser is in src/parse.rs:42.\n\nIntegrate these findings into the current work. Call get_subagent_result(\"subagent-7\") for the full transcript and file references if needed. Update your task list with task_list_update if any tasks changed state."},
        {"role": "agent_event", "content": "Something the engine said."}
    ]
}"#;

#[derive(Deserialize)]
struct ChatFile {
    messages: Vec<PersistedEntry>,
}

#[test]
fn old_agent_event_chats_load_as_reports_and_notices() {
    let file: ChatFile = serde_json::from_str(LEGACY_CHAT).unwrap();
    let ledger = Ledger::from_persisted(file.messages);
    assert_eq!(ledger.len(), 5);

    match &ledger.entries[3].kind {
        EntryKind::SubagentReport(report) => {
            assert_eq!(report.run_id, "subagent-7");
            assert_eq!(report.status, SubagentStatus::Success);
            assert!(report.summary.contains("src/parse.rs:42"));
            assert!(!report.summary.contains("Integrate these findings"));
        }
        other => panic!("expected a sub-agent report, got {other:?}"),
    }
    assert!(matches!(
        ledger.entries[4].kind,
        EntryKind::EngineNotice { .. }
    ));
    assert_eq!(ledger.system_prompt(), Some("You are an AI assistant."));
    assert_eq!(ledger.first_user_text(), Some("find the parser"));

    // Nothing from the old file projects as a system message past the first.
    let messages = project(&ledger, &ModelProfile::default());
    let system_count = messages
        .iter()
        .filter(|m| m.role == agent_chat_core::ChatRole::System)
        .count();
    assert_eq!(system_count, 1);
}

#[test]
fn new_entries_round_trip() {
    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("hi"));
    ledger.push(LedgerEntry::subagent_report(SubagentReport {
        run_id: "r1".to_string(),
        status: SubagentStatus::Error,
        summary: "failed".to_string(),
        refs: vec!["a.rs:1".to_string()],
    }));
    ledger.push(LedgerEntry::engine_notice("note"));
    ledger.push(LedgerEntry::summary("sum", 2..5));
    ledger.push(LedgerEntry::pinned(PinKind::Decision, "use tokio"));

    let json = serde_json::to_string(&ledger.to_persisted()).unwrap();
    let back = Ledger::from_persisted(serde_json::from_str(&json).unwrap());
    assert_eq!(back.len(), 6);
    match &back.entries[2].kind {
        EntryKind::SubagentReport(report) => {
            assert_eq!(report.run_id, "r1");
            assert_eq!(report.status, SubagentStatus::Error);
            assert_eq!(report.refs, vec!["a.rs:1".to_string()]);
        }
        other => panic!("unexpected {other:?}"),
    }
    match &back.entries[4].kind {
        EntryKind::Summary { covers, .. } => assert_eq!(covers, &(2..5)),
        other => panic!("unexpected {other:?}"),
    }
    assert!(matches!(
        back.entries[5].kind,
        EntryKind::Pinned {
            kind: PinKind::Decision,
            ..
        }
    ));
}

#[test]
fn plain_entries_keep_the_old_two_field_shape() {
    let ledger = Ledger::from_persisted(vec![PersistedEntry {
        role: "user".to_string(),
        content: "hi".to_string(),
        meta: None,
    }]);
    let json = serde_json::to_string(&ledger.to_persisted()).unwrap();
    assert_eq!(json, r#"[{"role":"user","content":"hi"}]"#);
}

#[test]
fn untrusted_provenance_survives_a_round_trip() {
    use crate::ledger::Source;
    use crate::taint::ToolClassifier;
    use std::path::Path;

    let mut ledger = Ledger::with_system_prompt("sys");
    ledger.push(LedgerEntry::user("hi"));
    let mut tool_note = LedgerEntry::engine_notice("page text");
    tool_note.source = Source::Tool {
        name: "fetch_url".to_string(),
    };
    ledger.push(tool_note);
    ledger.push(LedgerEntry::engine_notice("our own notice"));
    ledger.push(LedgerEntry::subagent_report(SubagentReport {
        run_id: "r1".to_string(),
        status: SubagentStatus::Success,
        summary: "report".to_string(),
        refs: vec![],
    }));

    let json = serde_json::to_string(&ledger.to_persisted()).unwrap();
    // Trusted entries keep the old shape; only untrusted ones gain a tag.
    assert!(json.contains(r#""untrusted":true"#));
    assert!(json.contains(r#"{"role":"engine_notice","content":"our own notice"}"#));

    let back = Ledger::from_persisted(serde_json::from_str(&json).unwrap());
    assert_eq!(
        back.entries[2].source,
        Source::Tool {
            name: "fetch_url".to_string()
        }
    );
    assert_eq!(back.entries[3].source, Source::Engine);
    assert!(matches!(back.entries[4].source, Source::Subagent { .. }));
    assert!(back.is_tainted(&ToolClassifier::new(), Path::new(".")));

    // A chat with only trusted entries is not tainted.
    let clean = Ledger::from_persisted(vec![PersistedEntry {
        role: "engine_notice".to_string(),
        content: "ours".to_string(),
        meta: None,
    }]);
    assert!(!clean.is_tainted(&ToolClassifier::new(), Path::new(".")));
}