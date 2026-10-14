//! Time travel: fork a ledger at any entry, snapshot it, and replay a
//! recorded run against a [`MockProvider`] to reproduce it.
//!
//! Because the ledger is typed and projection is deterministic, a recorded
//! ledger holds everything needed to re-run: the inputs (system prompt, user
//! turns, user/accepted pins) and the model's responses (assistant turns with
//! their tool calls). Replay feeds the responses back in order and re-runs the
//! loop, so any divergence is a runtime behaviour change.
//!
//! Limits: replay re-executes tools through the registry the caller supplies
//! (give it the same or a stubbed registry), and a run whose compaction made
//! a model-written summary needs the summary call in the script; replays
//! assume compaction falls back to the structured summary or does not occur.

use std::sync::Arc;

use agent_chat_core::ChatProvider;

use crate::{
    ledger::{EntryKind, Ledger, LedgerEntry, PinKind},
    profile::{ModelProfile, ToolCalling},
    run::{run_loop, CancelToken, EventSink, RunConfig},
    testing::{MockProvider, ScriptedStep},
    text_tools,
};

/// A frozen copy of a ledger, restorable later.
#[derive(Clone, Debug)]
pub struct LedgerSnapshot {
    pub ledger: Ledger,
    /// Entry count when the snapshot was taken.
    pub len: usize,
}

impl Ledger {
    /// A new ledger holding entries `0..=index` (the fork point is the entry
    /// the user clicked, inclusive), plus every live pin made after it.
    ///
    /// Pins are the user's durable intent and are appended wherever they were
    /// made, so a fork keeps them in their current state (edits and removals
    /// included). Everything else after `index` is dropped.
    pub fn fork_at(&self, index: usize) -> Ledger {
        let mut forked = self.clone();
        forked.truncate_keep_pins(index.saturating_add(1));
        forked
    }

    pub fn snapshot(&self) -> LedgerSnapshot {
        LedgerSnapshot {
            ledger: self.clone(),
            len: self.len(),
        }
    }

    /// Replace this ledger's contents with the snapshot's.
    pub fn restore(&mut self, snapshot: &LedgerSnapshot) {
        *self = snapshot.ledger.clone();
    }
}

fn is_input(entry: &LedgerEntry) -> bool {
    match &entry.kind {
        EntryKind::System { .. } | EntryKind::UserTurn { .. } => true,
        EntryKind::Pinned { kind, text } => {
            !matches!(kind, PinKind::FileTouched | PinKind::LiveState) && !text.trim().is_empty()
        }
        _ => false,
    }
}

/// The model's responses in `ledger`, in order, as a provider script.
/// Each assistant turn plus the tool exchanges that follow it is one step.
/// With `ToolCalling::None` the calls are rendered into the text, as the
/// model would have written them.
pub fn script_from_ledger(ledger: &Ledger, profile: &ModelProfile) -> Vec<ScriptedStep> {
    let mut steps: Vec<ScriptedStep> = Vec::new();
    let mut open = false;
    for entry in &ledger.entries {
        match &entry.kind {
            EntryKind::AssistantTurn { text } => {
                steps.push(ScriptedStep::say(text.clone()));
                open = true;
            }
            EntryKind::ToolExchange(exchange) if open => {
                if let Some(step) = steps.last_mut() {
                    step.tool_calls.push(exchange.call.clone());
                }
            }
            EntryKind::UserTurn { .. } => open = false,
            _ => {}
        }
    }
    if profile.tool_calling == ToolCalling::None {
        for step in &mut steps {
            for call in std::mem::take(&mut step.tool_calls) {
                if !step.text.is_empty() {
                    step.text.push('\n');
                }
                step.text.push_str(&text_tools::render_call(&call));
            }
        }
    }
    steps
}

/// One line per entry, stable across runs; what replay compares.
pub fn fingerprint(entry: &LedgerEntry) -> String {
    match &entry.kind {
        EntryKind::System { text } => format!("system|{text}"),
        EntryKind::UserTurn { text } => format!("user|{text}"),
        EntryKind::AssistantTurn { text } => format!("assistant|{text}"),
        EntryKind::ToolExchange(x) => format!(
            "tool|{}|{}|{}|{}|{}",
            // Text-protocol call ids come from a process-wide counter.
            if x.call.id.starts_with("text-call-") { "text-call" } else { x.call.id.as_str() },
            x.call.name, x.call.arguments_json, x.result.is_error, x.result.content
        ),
        EntryKind::SubagentReport(r) => format!(
            "report|{}|{}|{}|{}",
            r.run_id,
            r.status.as_str(),
            r.summary,
            r.refs.join(",")
        ),
        EntryKind::EngineNotice { text } => format!("notice|{text}"),
        EntryKind::Summary { text, .. } => format!("summary|{text}"),
        EntryKind::Pinned { kind, text } => format!("pin|{}|{text}", kind.label()),
    }
}

/// Where two ledgers first differ, ignoring pin position (pins are compared
/// as a set, since steward pins land at different positions).
pub fn first_divergence(recorded: &Ledger, replayed: &Ledger) -> Option<String> {
    let strip = |ledger: &Ledger| -> Vec<String> {
        ledger
            .entries
            .iter()
            .filter(|e| !matches!(e.kind, EntryKind::Pinned { .. }))
            .map(fingerprint)
            .collect()
    };
    let (a, b) = (strip(recorded), strip(replayed));
    for i in 0..a.len().max(b.len()) {
        if a.get(i) != b.get(i) {
            return Some(format!(
                "entry {i}: recorded {:?} vs replayed {:?}",
                a.get(i),
                b.get(i)
            ));
        }
    }
    let pins = |ledger: &Ledger| {
        let mut p: Vec<String> = ledger
            .pins()
            .into_iter()
            .filter(|p| p.kind != PinKind::FileTouched)
            .map(|p| format!("{}|{}", p.kind.label(), p.text))
            .collect();
        p.sort();
        p
    };
    (pins(recorded) != pins(replayed)).then(|| "pins differ".to_string())
}

/// The result of [`replay`].
pub struct Replay {
    pub ledger: Ledger,
    /// Requests the mock provider received, in order.
    pub requests: Vec<agent_chat_core::ChatRequest>,
    /// `None` when the replay reproduced the recording.
    pub divergence: Option<String>,
}

impl Replay {
    pub fn reproduced(&self) -> bool {
        self.divergence.is_none()
    }
}

/// Re-run `recorded` against a mock provider. `make_config` builds the run
/// configuration for each user turn around the supplied provider (set the
/// profile, tools and registry there). Inputs are taken from the recording
/// (system prompt, user turns, user pins); the model's responses come from its
/// assistant turns.
pub fn replay(
    recorded: &Ledger,
    profile: &ModelProfile,
    mut make_config: impl FnMut(Arc<dyn ChatProvider>) -> RunConfig,
    sink: EventSink,
) -> Replay {
    let mut ledger = Ledger::new();
    let mut requests = Vec::new();

    // Split the recording into segments, each starting at a user turn.
    let mut segments: Vec<Vec<&LedgerEntry>> = vec![Vec::new()];
    for entry in &recorded.entries {
        if matches!(entry.kind, EntryKind::UserTurn { .. }) && !segments.last().unwrap().is_empty()
        {
            segments.push(Vec::new());
        }
        segments.last_mut().unwrap().push(entry);
    }

    for segment in segments {
        let seg_ledger = Ledger {
            entries: segment.iter().map(|e| (*e).clone()).collect(),
        };
        for entry in segment.iter().filter(|e| is_input(e)) {
            ledger.push((*entry).clone());
        }
        let steps = script_from_ledger(&seg_ledger, profile);
        if steps.is_empty() {
            continue;
        }
        let provider = MockProvider::scripted(steps);
        ledger = run_loop(
            make_config(provider.clone()),
            ledger,
            CancelToken::new(),
            sink.clone(),
        );
        requests.extend(provider.requests());
    }

    let divergence = first_divergence(recorded, &ledger);
    Replay {
        ledger,
        requests,
        divergence,
    }
}
