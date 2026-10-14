//! Pinned requirements survive long conversations and compaction.

use agent_chat_core::{ChatRequest, ChatRole};
use agent_runtime::{
    run_loop,
    testing::{MockProvider, ScriptedStep},
    CancelToken, EntryKind, Ledger, LedgerEntry, ModelProfile, PinAuthor, PinKind, RunEvent,
};

use crate::common::{config, sink};

fn is_summary(request: &ChatRequest) -> bool {
    request
        .messages
        .first()
        .is_some_and(|m| m.content.contains("conversation summarizer"))
}

#[test]
fn pinned_requirements_survive_200_turns_at_a_4k_profile() {
    let provider = MockProvider::from_fn(|request, index| {
        let reply = if is_summary(request) {
            "The user asked for many small descriptions. ".repeat(6)
        } else {
            format!("Reply {index}: {}", "The assistant answers at some length. ".repeat(8))
        };
        Ok(ScriptedStep::say(reply).into_response())
    });
    let profile = ModelProfile::new(4_096, true);
    let mut ledger = Ledger::with_system_prompt("You are a helpful assistant.");
    ledger.add_pin(PinKind::Requirement, "Never touch the vendored submodules.", PinAuthor::User);
    ledger.add_pin(PinKind::Constraint, "Only use stable Rust.", PinAuthor::User);

    let mut compactions = 0usize;
    for turn in 0..200 {
        ledger.push(LedgerEntry::user(format!(
            "Turn {turn}: describe step {turn}. {}",
            "More detail please. ".repeat(10)
        )));
        let (sink, events) = sink();
        ledger = run_loop(config(provider.clone(), profile), ledger, CancelToken::new(), sink);
        let events = events.lock().unwrap();
        assert!(matches!(events.last(), Some(RunEvent::Finished(Ok(_)))), "turn {turn}");
        compactions += events
            .iter()
            .filter(|e| matches!(e, RunEvent::ContextCompacted(_)))
            .count();
    }
    assert!(compactions >= 10, "compaction should have run, got {compactions}");
    assert!(ledger.entries.iter().any(|e| matches!(e.kind, EntryKind::Summary { .. })));

    let mains: Vec<ChatRequest> = provider
        .requests()
        .into_iter()
        .filter(|r| !is_summary(r))
        .collect();
    assert_eq!(mains.len(), 200);
    for (turn, request) in mains.iter().enumerate() {
        let system = &request.messages[0];
        assert_eq!(system.role, ChatRole::System);
        assert!(system.content.contains("- Never touch the vendored submodules."), "turn {turn}");
        assert!(system.content.contains("- Only use stable Rust."), "turn {turn}");
    }
}
