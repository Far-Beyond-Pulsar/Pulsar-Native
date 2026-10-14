//! Headless agent runtime: typed ledger, role projector and the agent loop.
//! No UI dependencies; the chat panel subscribes to [`RunEvent`]s.

mod compaction;
mod ledger;
mod persist;
mod preview;
mod profile;
mod project;
mod run;

#[cfg(test)]
mod persist_tests;
#[cfg(test)]
mod project_tests;

pub use compaction::{
    compact_ledger, compact_messages, summarize_compaction, summarize_dropped, Compaction,
};
pub use ledger::{
    EntryKind, Ledger, LedgerEntry, PinKind, Source, SubagentReport, SubagentStatus, ToolExchange,
    ToolResult,
};
pub use persist::{PersistedEntry, PersistedMeta};
pub use preview::format_tool_result_preview;
pub use profile::{ModelProfile, ToolCalling};
pub use project::{
    notice_envelope, project, project_indexed, report_envelope, Projected, DATA_ENVELOPE_RULE,
};
pub use run::{
    run_loop, spawn_run, CancelToken, EventSink, RunConfig, RunEvent, ThinkSplitter, UiReply,
    CANCELLED_MESSAGE,
};
