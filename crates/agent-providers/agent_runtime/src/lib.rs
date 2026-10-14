//! Headless agent runtime: typed ledger, role projector and the agent loop.
//! No UI dependencies; the chat panel subscribes to [`RunEvent`]s.

mod budget_guard;
mod checkpoint;
mod budget;
mod compaction;
mod confirm;
mod delegation;
mod harness;
mod ledger;
mod output_cap;
mod persist;
mod preview;
mod profile;
mod project;
mod router;
mod run;
mod taint;
mod summary;
mod text_tools;
mod run_store;

#[cfg(test)]
mod persist_tests;
#[cfg(test)]
mod project_tests;

pub use budget_guard::{BudgetError, BudgetGuard, RunBudget, SpawnPermit};
pub use checkpoint::{
    affected_paths, has_checkpoint, revert_checkpoint, CheckpointStore, RevertReport,
    CHECKPOINT_DIR,
};
pub use confirm::{ConfirmDecision, ConfirmReply, SafetyConfig};
pub use budget::{usage_from_raw, Budget, Calibrator, TokenEstimator, TokenUsage};
pub use compaction::{
    compact_ledger, compact_ledger_tokens, compact_messages, summarize_compaction,
    summarize_dropped, Compaction,
};
pub use output_cap::{
    cap_output, fetch_output_definition, run_fetch_output, Capped, OutputStore, FETCH_OUTPUT,
};
pub use router::{
    apply_compact_system, categorize, shorten_definition, Category, RouteContext, ToolRouter,
    LITE_SYSTEM_PROMPT, MORE_TOOLS,
};
pub use summary::summarize_into_ledger;
pub use text_tools::{
    constrained_call_format, parse as parse_text_tool_calls, protocol_prompt, render_call,
    tool_call_grammar, tool_call_schema, FenceFilter, Parsed as ParsedToolCalls,
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
pub use profile::{
    cached_overrides, infer_context_tokens, parse_param_billions, provider_json_mode,
    resolve_profile, resolve_with_store, JsonMode, ModelProfile, ProfileOverride, ProfileOverrides,
    ResolveInput, Thinking, Tier, TokenizerHint, ToolCalling, LITE_CONTEXT_LIMIT,
    UNKNOWN_CONTEXT_TOKENS,
};
pub use project::{
    notice_envelope, project, project_indexed, report_envelope, Projected, DATA_ENVELOPE_RULE,
};
pub use run::{
    run_loop, spawn_run, CancelToken, EventSink, LedgerHook, RunConfig, RunEvent, ThinkSplitter,
    UiReply, CANCELLED_MESSAGE,
};
pub use taint::{
    entry_taints, is_untrusted_path, ConfirmPolicy, TaintTracker, ToolClass, ToolClassifier,
};
pub use run_store::{ChildReport, RunRecord, RunStatus, RunStore};
