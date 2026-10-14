//! On-disk form of the ledger, kept readable by files from before the ledger
//! existed.
//!
//! Chat files store `{role, content}` pairs. New entry kinds add their own
//! role strings plus an optional `meta` block; old files only ever contain
//! `system`, `user`, `assistant` and (from the former `ChatRole::AgentEvent`)
//! `agent_event`, which loads as a sub-agent report or an engine notice.

use serde::{Deserialize, Serialize};

use crate::output_cap::{cap_output, OutputStore};
use crate::ledger::{
    EntryKind, Ledger, LedgerEntry, PinKind, Source, SubagentReport, SubagentStatus,
};

/// Longest tool result kept verbatim in a chat file.
pub const MAX_PERSISTED_RESULT_CHARS: usize = 12_000;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub covers: Option<(usize, usize)>,
    /// Content came from a tool, the web or a sub-agent. Reloading restores
    /// the provenance so the run's taint survives a restart.
    #[serde(default, skip_serializing_if = "is_false")]
    pub untrusted: bool,
    /// Name of the tool that produced an untrusted entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// A pin the user wrote or accepted (as opposed to one the engine keeps).
    #[serde(default, skip_serializing_if = "is_false")]
    pub user_pinned: bool,
    /// Tool exchange: id of the call the result answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_id: Option<String>,
    /// Tool exchange: the call's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// Tool exchange: the tool reported an error.
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedEntry {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<PersistedMeta>,
}

impl PersistedEntry {
    pub fn into_entry(self) -> LedgerEntry {
        let meta = self.meta.unwrap_or_default();
        let untrusted_tool = (meta.untrusted || self.role == "tool")
            .then(|| meta.tool.clone().unwrap_or_else(|| "tool".to_string()));
        let mut entry = match self.role.as_str() {
            "system" => LedgerEntry::system(self.content),
            "user" => LedgerEntry::user(self.content),
            "agent_event" => legacy_agent_event(&self.content),
            "subagent_report" => LedgerEntry::subagent_report(SubagentReport {
                run_id: meta.run_id.unwrap_or_default(),
                status: SubagentStatus::parse(meta.status.as_deref().unwrap_or("success")),
                summary: self.content,
                refs: meta.refs,
            }),
            "engine_notice" => LedgerEntry::engine_notice(self.content),
            "tool_exchange" => LedgerEntry::tool_exchange(
                agent_chat_core::ToolCall {
                    id: meta.call_id.clone().unwrap_or_default(),
                    name: meta.tool.clone().unwrap_or_else(|| "tool".to_string()),
                    arguments_json: meta.args.clone().unwrap_or(serde_json::Value::Null),
                },
                crate::ledger::ToolResult {
                    content: self.content,
                    is_error: meta.is_error,
                },
            ),
            // A bare tool message has no call to attach to; keep it as data.
            "tool" => LedgerEntry::engine_notice(self.content),
            "summary" => {
                let (start, end) = meta.covers.unwrap_or((0, 0));
                LedgerEntry::summary(self.content, start..end)
            }
            "pinned" => {
                let mut pin = LedgerEntry::pinned(
                    parse_pin_kind(meta.pin_kind.as_deref().unwrap_or("")),
                    self.content,
                );
                if meta.user_pinned {
                    pin.source = Source::User;
                }
                pin
            }
            // Unknown roles were read as assistant text before the ledger.
            _ => LedgerEntry::assistant(self.content),
        };
        // Tool output stays untrusted after a reload.
        if let (Some(name), EntryKind::EngineNotice { .. }) = (untrusted_tool, &entry.kind) {
            entry.source = Source::Tool { name };
        }
        entry
    }
}

impl LedgerEntry {
    /// `None` for removed-pin tombstones. Tool results longer than
    /// [`MAX_PERSISTED_RESULT_CHARS`] are cut to head + tail.
    pub fn to_persisted(&self) -> Option<PersistedEntry> {
        let (role, content, meta) = match &self.kind {
            EntryKind::System { text } => ("system", text.clone(), None),
            EntryKind::UserTurn { text } => ("user", text.clone(), None),
            EntryKind::AssistantTurn { text } => ("assistant", text.clone(), None),
            EntryKind::ToolExchange(exchange) => (
                "tool_exchange",
                cap_output(
                    &exchange.result.content,
                    MAX_PERSISTED_RESULT_CHARS,
                    OutputStore::global(),
                )
                .text,
                Some(PersistedMeta {
                    untrusted: true,
                    tool: Some(exchange.call.name.clone()),
                    call_id: Some(exchange.call.id.clone()),
                    args: Some(exchange.call.arguments_json.clone()),
                    is_error: exchange.result.is_error,
                    ..PersistedMeta::default()
                }),
            ),
            EntryKind::SubagentReport(report) => (
                "subagent_report",
                report.summary.clone(),
                Some(PersistedMeta {
                    run_id: Some(report.run_id.clone()),
                    status: Some(report.status.as_str().to_string()),
                    refs: report.refs.clone(),
                    untrusted: true,
                    ..PersistedMeta::default()
                }),
            ),
            EntryKind::EngineNotice { text } => (
                "engine_notice",
                text.clone(),
                match &self.source {
                    Source::Tool { name } => Some(PersistedMeta {
                        untrusted: true,
                        tool: Some(name.clone()),
                        ..PersistedMeta::default()
                    }),
                    _ => None,
                },
            ),
            EntryKind::Summary { text, covers } => (
                "summary",
                text.clone(),
                Some(PersistedMeta {
                    covers: Some((covers.start, covers.end)),
                    ..PersistedMeta::default()
                }),
            ),
            // Removed pins stay as empty tombstones in memory (so ledger
            // indexes do not shift); they are not worth saving.
            EntryKind::Pinned { text, .. } if text.trim().is_empty() => return None,
            EntryKind::Pinned { kind, text } => (
                "pinned",
                text.clone(),
                Some(PersistedMeta {
                    pin_kind: Some(kind.label().to_string()),
                    user_pinned: self.source == Source::User,
                    ..PersistedMeta::default()
                }),
            ),
        };
        Some(PersistedEntry {
            role: role.to_string(),
            content,
            meta,
        })
    }
}

impl Ledger {
    pub fn from_persisted(entries: Vec<PersistedEntry>) -> Ledger {
        Ledger {
            entries: entries.into_iter().map(PersistedEntry::into_entry).collect(),
        }
    }

    pub fn to_persisted(&self) -> Vec<PersistedEntry> {
        self.entries
            .iter()
            .filter_map(LedgerEntry::to_persisted)
            .collect()
    }
}

fn parse_pin_kind(raw: &str) -> PinKind {
    PinKind::parse(raw).unwrap_or(PinKind::LiveState)
}

/// The text the panel used to write for a finished sub-agent:
///
/// ```text
/// Sub-agent '<name>' (id=<id>) completed — status: <status>.
///
/// Findings preview:
/// <preview>
///
/// Integrate these findings into the current work. ...
/// ```
///
/// The trailing instruction is dropped; anything that does not match the
/// pattern becomes a plain engine notice.
fn legacy_agent_event(content: &str) -> LedgerEntry {
    let parsed = (|| {
        let rest = content.strip_prefix("Sub-agent '")?;
        let (name, rest) = rest.split_once("' (id=")?;
        let (run_id, rest) = rest.split_once(')')?;
        let status = rest
            .split_once("status: ")
            .map(|(_, tail)| tail.chars().take_while(|c| c.is_ascii_alphabetic()).collect())
            .unwrap_or_else(|| "success".to_string());
        let preview = content.split_once("Findings preview:\n").map(|(_, tail)| {
            tail.split("\n\nIntegrate these findings")
                .next()
                .unwrap_or(tail)
                .trim()
                .to_string()
        });
        let status = SubagentStatus::parse(&status);
        let mut summary = format!("Sub-agent '{name}' finished with status {}.", status.as_str());
        if let Some(preview) = preview.filter(|p| !p.is_empty()) {
            summary.push_str("\n\n");
            summary.push_str(&preview);
        }
        Some(SubagentReport {
            run_id: run_id.to_string(),
            status,
            summary,
            refs: Vec::new(),
        })
    })();

    match parsed {
        Some(report) => LedgerEntry::subagent_report(report),
        None => LedgerEntry::engine_notice(content),
    }
}
