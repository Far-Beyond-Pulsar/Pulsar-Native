//! Headless agent runtime: typed ledger, role projector and the agent loop.
//! No UI dependencies; the chat panel subscribes to [`RunEvent`]s.

mod compaction;
mod delegation;
mod harness;
mod ledger;
mod persist;
mod preview;
mod profile;
mod project;
mod run;
mod run_store;

#[cfg(test)]
mod persist_tests;
#[cfg(test)]
mod project_tests;

pub use compaction::{
    compact_ledger, compact_messages, summarize_compaction, summarize_dropped, Compaction,
};
pub use delegation::{
    deliver_pending, parse_report, report_value, subagent_report_for, DelegationHandle,
    DelegationHost, DelegationLimits, SpawnError, ROOT_RUN_ID,
};
pub use harness::{
    builtin_harnesses, project_agents_dir, Harness, HarnessOrigin, HarnessPermissions,
    HarnessRegistry, ModelPolicy,
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
    run_loop, spawn_run, CancelToken, EventSink, LedgerHook, RunConfig, RunEvent, ThinkSplitter,
    UiReply, CANCELLED_MESSAGE,
};
pub use run_store::{ChildReport, RunRecord, RunStatus, RunStore};
