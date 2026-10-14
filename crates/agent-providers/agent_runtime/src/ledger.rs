//! The typed ledger: the source of truth for one conversation.
//!
//! Every entry records *who said it* (`Source`). Roles are decided later, and
//! only, by [`crate::project`].

use std::ops::Range;

use agent_chat_core::ToolCall;

/// Who produced a ledger entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The human at the keyboard.
    User,
    /// The model of this run.
    Assistant,
    /// The system prompt.
    System,
    /// The engine itself (notices, compaction, live state).
    Engine,
    /// Output of a tool; untrusted data.
    Tool { name: String },
    /// A sub-agent run; untrusted data.
    Subagent { run_id: String },
}

impl Source {
    /// Content from this source is data to the model, never an instruction.
    pub fn is_untrusted(&self) -> bool {
        matches!(self, Source::Tool { .. } | Source::Subagent { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubagentStatus {
    Success,
    Error,
    Cancelled,
}

impl SubagentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SubagentStatus::Success => "success",
            SubagentStatus::Error => "error",
            SubagentStatus::Cancelled => "cancelled",
        }
    }

    /// Unknown strings read as `Success`, matching how the panel treated them.
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "error" => SubagentStatus::Error,
            "cancelled" => SubagentStatus::Cancelled,
            _ => SubagentStatus::Success,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubagentReport {
    pub run_id: String,
    pub status: SubagentStatus,
    pub summary: String,
    /// `file:line` references and pointers to fuller output.
    pub refs: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
}

#[derive(Clone, Debug)]
pub struct ToolExchange {
    pub call: ToolCall,
    pub result: ToolResult,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinKind {
    Requirement,
    Constraint,
    Decision,
    OpenQuestion,
    FileTouched,
    /// Task list and active sub-agents, rebuilt for each request.
    LiveState,
}

impl PinKind {
    pub fn label(self) -> &'static str {
        match self {
            PinKind::Requirement => "requirement",
            PinKind::Constraint => "constraint",
            PinKind::Decision => "decision",
            PinKind::OpenQuestion => "open_question",
            PinKind::FileTouched => "file_touched",
            PinKind::LiveState => "live_state",
        }
    }

    /// Heading used in the pinned block and the panel strip.
    pub fn title(self) -> &'static str {
        match self {
            PinKind::Requirement => "Requirements",
            PinKind::Constraint => "Constraints",
            PinKind::Decision => "Decisions",
            PinKind::OpenQuestion => "Open questions",
            PinKind::FileTouched => "Files touched",
            PinKind::LiveState => "Live state",
        }
    }

    /// Parses a label, a title or a loose spelling.
    pub fn parse(raw: &str) -> Option<PinKind> {
        match raw.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "requirement" | "requirements" => Some(PinKind::Requirement),
            "constraint" | "constraints" => Some(PinKind::Constraint),
            "decision" | "decisions" => Some(PinKind::Decision),
            "open_question" | "open_questions" | "question" => Some(PinKind::OpenQuestion),
            "file_touched" | "files_touched" | "file" => Some(PinKind::FileTouched),
            "live_state" => Some(PinKind::LiveState),
            _ => None,
        }
    }

    /// The kinds the user sees, in display order.
    pub const DURABLE: [PinKind; 5] = [
        PinKind::Requirement,
        PinKind::Constraint,
        PinKind::Decision,
        PinKind::OpenQuestion,
        PinKind::FileTouched,
    ];

    /// Kinds that bind the agent; never elided from the pinned block.
    pub fn is_binding(self) -> bool {
        matches!(self, PinKind::Requirement | PinKind::Constraint)
    }
}

#[derive(Clone, Debug)]
pub enum EntryKind {
    /// The system prompt. Only the projector places it.
    System { text: String },
    UserTurn { text: String },
    AssistantTurn { text: String },
    ToolExchange(ToolExchange),
    SubagentReport(SubagentReport),
    EngineNotice { text: String },
    /// Replaces earlier entries that were compacted away. `covers` is the
    /// span of pre-compaction entry indexes that were folded in.
    Summary { text: String, covers: Range<usize> },
    Pinned { kind: PinKind, text: String },
}

#[derive(Clone, Debug)]
pub struct LedgerEntry {
    pub source: Source,
    pub kind: EntryKind,
}

impl LedgerEntry {
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            source: Source::System,
            kind: EntryKind::System { text: text.into() },
        }
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self {
            source: Source::User,
            kind: EntryKind::UserTurn { text: text.into() },
        }
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self {
            source: Source::Assistant,
            kind: EntryKind::AssistantTurn { text: text.into() },
        }
    }

    pub fn tool_exchange(call: ToolCall, result: ToolResult) -> Self {
        Self {
            source: Source::Tool {
                name: call.name.clone(),
            },
            kind: EntryKind::ToolExchange(ToolExchange { call, result }),
        }
    }

    pub fn subagent_report(report: SubagentReport) -> Self {
        Self {
            source: Source::Subagent {
                run_id: report.run_id.clone(),
            },
            kind: EntryKind::SubagentReport(report),
        }
    }

    pub fn engine_notice(text: impl Into<String>) -> Self {
        Self {
            source: Source::Engine,
            kind: EntryKind::EngineNotice { text: text.into() },
        }
    }

    pub fn summary(text: impl Into<String>, covers: Range<usize>) -> Self {
        Self {
            source: Source::Engine,
            kind: EntryKind::Summary {
                text: text.into(),
                covers,
            },
        }
    }

    pub fn pinned(kind: PinKind, text: impl Into<String>) -> Self {
        Self {
            source: Source::Engine,
            kind: EntryKind::Pinned {
                kind,
                text: text.into(),
            },
        }
    }

    /// Character count of the text this entry contributes to a request.
    pub fn char_len(&self) -> usize {
        match &self.kind {
            EntryKind::System { text }
            | EntryKind::UserTurn { text }
            | EntryKind::AssistantTurn { text }
            | EntryKind::EngineNotice { text }
            | EntryKind::Summary { text, .. }
            | EntryKind::Pinned { text, .. } => text.len(),
            EntryKind::ToolExchange(exchange) => exchange.result.content.len(),
            EntryKind::SubagentReport(report) => report.summary.len(),
        }
    }
}

/// An ordered list of entries. Indexes are stable until the ledger is
/// truncated or compacted; the panel keys its display items on them.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    pub entries: Vec<LedgerEntry>,
}

impl Ledger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_system_prompt(text: impl Into<String>) -> Self {
        Self {
            entries: vec![LedgerEntry::system(text)],
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn push(&mut self, entry: LedgerEntry) -> usize {
        self.entries.push(entry);
        self.entries.len() - 1
    }

    pub fn truncate(&mut self, len: usize) {
        self.entries.truncate(len);
    }

    /// The first `len` entries as a new ledger (for forking a chat).
    pub fn prefix(&self, len: usize) -> Ledger {
        Ledger {
            entries: self.entries[..len.min(self.entries.len())].to_vec(),
        }
    }

    pub fn char_len(&self) -> usize {
        self.entries.iter().map(LedgerEntry::char_len).sum()
    }

    pub fn system_prompt(&self) -> Option<&str> {
        self.entries.iter().find_map(|entry| match &entry.kind {
            EntryKind::System { text } => Some(text.as_str()),
            _ => None,
        })
    }

    /// Replace the first system prompt, or insert one at the front.
    pub fn set_system_prompt(&mut self, new_text: impl Into<String>) {
        let new_text = new_text.into();
        for entry in &mut self.entries {
            if let EntryKind::System { text } = &mut entry.kind {
                *text = new_text;
                return;
            }
        }
        self.entries.insert(0, LedgerEntry::system(new_text));
    }

    /// The text of the assistant turn at `ix`, if that entry is one.
    pub fn assistant_text_mut(&mut self, ix: usize) -> Option<&mut String> {
        match self.entries.get_mut(ix).map(|entry| &mut entry.kind) {
            Some(EntryKind::AssistantTurn { text }) => Some(text),
            _ => None,
        }
    }

    /// Title source for a chat: the first user turn.
    pub fn first_user_text(&self) -> Option<&str> {
        self.entries.iter().find_map(|entry| match &entry.kind {
            EntryKind::UserTurn { text } => Some(text.as_str()),
            _ => None,
        })
    }
}
