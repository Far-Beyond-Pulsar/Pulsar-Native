//! The agent loop: request, stream, run tools, compact, repeat. It owns no UI;
//! everything the outside world needs is a [`RunEvent`].

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use agent_chat_core::{
    ChatProvider, ChatRequest, ChatResponse, ResponseFormat, ToolCall, ToolDefinition,
};
use agent_chat_tools::{PulsarToolExtras, ToolContext, ToolRegistry};

use crate::budget::{usage_from_raw, Budget, Calibrator, TokenEstimator};
use crate::compaction::{compact_ledger, compact_ledger_tokens};
use crate::confirm::{ConfirmReply, SafetyConfig, SafetyState};
use crate::ledger::{EntryKind, Ledger, LedgerEntry, SubagentStatus, ToolResult};
use crate::output_cap::{
    cap_output, fetch_output_definition, run_fetch_output, OutputStore, FETCH_OUTPUT,
};
use crate::preview::format_tool_result_preview;
use crate::profile::{ModelProfile, ToolCalling};
use crate::project::project;
use crate::router::{
    shorten_definition, Category, RouteContext, ToolRouter, LITE_SYSTEM_PROMPT, MORE_TOOLS,
};
use crate::summary::summarize_into_ledger;
use crate::text_tools::{self, FenceFilter};

pub const CANCELLED_MESSAGE: &str = "Request cancelled.";

/// Cooperative cancellation. Cloning shares the flag. A [`child`] token is
/// cancelled when its parent is, but cancelling it leaves the parent alone.
///
/// [`child`]: CancelToken::child
#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    parent: Option<Box<CancelToken>>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token that also reports cancelled once `self` is.
    pub fn child(&self) -> CancelToken {
        CancelToken {
            flag: Arc::new(AtomicBool::new(false)),
            parent: Some(Box::new(self.clone())),
        }
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
            || self.parent.as_ref().is_some_and(|parent| parent.is_cancelled())
    }
}

/// How the UI answers a request the loop made on its behalf. The loop blocks
/// in [`UiReply::wait`] until the UI calls [`UiReply::reply`].
#[derive(Clone, Default)]
pub struct UiReply(Arc<(Mutex<Option<Result<(), String>>>, Condvar)>);

impl UiReply {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reply(&self, result: Result<(), String>) {
        let (lock, cvar) = &*self.0;
        if let Ok(mut guard) = lock.lock() {
            *guard = Some(result);
            cvar.notify_all();
        }
    }

    pub fn wait(&self) -> Result<(), String> {
        let (lock, cvar) = &*self.0;
        let mut guard = lock
            .lock()
            .map_err(|_| "UI reply mutex poisoned".to_string())?;
        while guard.is_none() {
            guard = cvar
                .wait(guard)
                .map_err(|_| "UI reply mutex poisoned".to_string())?;
        }
        guard
            .take()
            .unwrap_or_else(|| Err("UI reply signaled without a result".to_string()))
    }
}

pub enum RunEvent {
    /// Assistant prose, appended to the current streaming bubble.
    Chunk(String),
    /// A `<think>` tag was opened.
    ThinkingStarted,
    /// Incremental thinking content.
    ThinkingChunk(String),
    /// `</think>` was reached.
    ThinkingDone,
    /// The model returned tool calls; they are about to run.
    ToolCallGroup(Vec<ToolCall>),
    ToolCallResult {
        id: String,
        tool_name: String,
        result_preview: String,
        result_full: String,
        is_error: bool,
    },
    /// Old entries were folded into a summary to stay within context.
    ContextCompacted(String),
    /// A tool wants a file opened in the editor; the panel must reply.
    OpenFile(PathBuf, UiReply),
    /// A tool wants an open editor tab activated; the panel must reply.
    ActivateOpenEditor(usize, UiReply),
    /// A write/external tool call needs the user's approval; the panel must
    /// call `reply.approve()` or `reply.deny()`. The tool does not run until
    /// then.
    ConfirmRequest {
        id: String,
        tool: String,
        args: serde_json::Value,
        /// Untrusted content is in the run's context.
        tainted: bool,
        reply: ConfirmReply,
    },
    /// The first file of this run was snapshotted; `revert_checkpoint(run_id)`
    /// can now undo the run's edits.
    CheckpointCreated { run_id: String },
    /// A sub-agent run was created. `parent_id` is the spawning run
    /// (`"main"` for the chat itself).
    ChildSpawned {
        run_id: String,
        parent_id: String,
        harness: String,
        task: String,
        depth: u32,
        background: bool,
    },
    /// A running sub-agent did something worth showing ("running list_files").
    ChildProgress { run_id: String, note: String },
    /// A sub-agent reached a terminal state; `summary` is its report summary.
    ChildFinished {
        run_id: String,
        status: SubagentStatus,
        summary: String,
    },
    /// Always the last event, exactly once.
    Finished(Result<ChatResponse, String>),
}

pub type EventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;

/// Called with the run's ledger before each request.
pub type LedgerHook = Arc<dyn Fn(&mut Ledger) + Send + Sync>;

#[derive(Clone)]
pub struct RunConfig {
    pub provider: Arc<dyn ChatProvider>,
    pub model: String,
    /// Model used for compaction summaries.
    pub compact_model: String,
    pub profile: ModelProfile,
    pub tools: Vec<ToolDefinition>,
    pub tool_registry: ToolRegistry,
    pub workspace_root: PathBuf,
    /// Plugin bridge, editor query and sub-agent executor. The loop fills in
    /// the open-file and activate-editor requests itself.
    pub tool_extras: PulsarToolExtras,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Character budgets, used only while the profile's context window is
    /// unknown (`context_tokens == 0`). Known windows are budgeted in tokens.
    pub context_budget_chars: usize,
    pub summary_char_budget: usize,
    pub max_iterations: u32,
    pub safety: SafetyConfig,
    /// Constrained-decoding hook, passed to every request.
    pub response_format: Option<ResponseFormat>,
    /// Replaces the ledger's system prompt for Lite profiles. `None` keeps it.
    pub lite_system_prompt: Option<String>,
    /// Overrides how many domain tools the Lite router exposes per turn.
    pub router_limit: Option<usize>,
    /// Makes the delegation tools available to this run; see
    /// [`crate::delegation`]. `run_loop` consumes it.
    pub delegation: Option<crate::delegation::DelegationHandle>,
    /// Runs before every request (delivers finished background children).
    pub pre_request_hook: Option<LedgerHook>,
}

impl RunConfig {
    /// Defaults match what the chat panel has always sent.
    pub fn new(
        provider: Arc<dyn ChatProvider>,
        model: impl Into<String>,
        tool_registry: ToolRegistry,
        workspace_root: PathBuf,
    ) -> Self {
        let model = model.into();
        Self {
            provider,
            compact_model: model.clone(),
            model,
            profile: ModelProfile::default(),
            tools: Vec::new(),
            tool_registry,
            workspace_root,
            tool_extras: PulsarToolExtras::default(),
            temperature: Some(0.2),
            top_p: Some(1.0),
            max_tokens: Some(8192),
            context_budget_chars: 24_000,
            summary_char_budget: 2_400,
            max_iterations: 50,
            safety: SafetyConfig::default(),
            response_format: None,
            lite_system_prompt: Some(LITE_SYSTEM_PROMPT.to_string()),
            router_limit: None,
            delegation: None,
            pre_request_hook: None,
        }
    }

    /// Adopt `profile`, including its sampling defaults.
    pub fn with_profile(mut self, profile: ModelProfile) -> Self {
        self.temperature = Some(profile.temperature);
        self.max_tokens = Some(profile.max_tokens);
        self.profile = profile;
        self
    }
}

/// Run on a new thread. The handle yields the final ledger.
pub fn spawn_run(
    config: RunConfig,
    ledger: Ledger,
    cancel: CancelToken,
    sink: EventSink,
) -> JoinHandle<Ledger> {
    std::thread::spawn(move || run_loop(config, ledger, cancel, sink))
}

/// Drive `ledger` to completion. Emits [`RunEvent::Finished`] exactly once,
/// last, and returns the ledger including every tool exchange.
pub fn run_loop(
    mut config: RunConfig,
    mut ledger: Ledger,
    cancel: CancelToken,
    sink: EventSink,
) -> Ledger {
    if let Some(handle) = config.delegation.take() {
        crate::delegation::install(handle, &mut config, &ledger, &cancel);
    }
    // Lite profiles run on the compact system prompt; the caller gets its
    // own prompt back.
    let original_prompt = swap_in_lite_prompt(&config, &mut ledger);
    let outcome = drive(&config, &mut ledger, &cancel, &sink);
    if let Some(prompt) = original_prompt {
        ledger.set_system_prompt(prompt);
    }
    sink(RunEvent::Finished(outcome));
    ledger
}

fn swap_in_lite_prompt(config: &RunConfig, ledger: &mut Ledger) -> Option<String> {
    if !config.profile.is_lite() {
        return None;
    }
    let compact = config.lite_system_prompt.as_deref()?;
    let original = ledger.system_prompt()?.to_string();
    if original.len() <= compact.len() {
        return None;
    }
    ledger.set_system_prompt(compact);
    Some(original)
}

/// Inputs the router reads from the ledger: the last user message, the live
/// task state and the most recent tool names.
fn route_inputs(ledger: &Ledger) -> (String, String, Vec<String>) {
    let mut user = String::new();
    let mut tasks = String::new();
    let mut recent: Vec<String> = Vec::new();
    for entry in &ledger.entries {
        match &entry.kind {
            EntryKind::UserTurn { text } => user = text.clone(),
            EntryKind::Pinned { kind, text } if *kind == crate::ledger::PinKind::LiveState => {
                tasks = text.clone();
            }
            EntryKind::ToolExchange(exchange) => recent.push(exchange.call.name.clone()),
            _ => {}
        }
    }
    let keep_from = recent.len().saturating_sub(6);
    (user, tasks, recent.split_off(keep_from))
}

fn ledger_mentions_refs(ledger: &Ledger) -> bool {
    ledger.entries.iter().any(|entry| match &entry.kind {
        EntryKind::ToolExchange(exchange) => exchange.result.content.contains("fetch_output ref="),
        _ => false,
    })
}

/// The tools to advertise this turn.
fn plan_tools(
    config: &RunConfig,
    ledger: &Ledger,
    router: Option<&ToolRouter>,
    unlocked: &[Category],
    limit: usize,
) -> Vec<ToolDefinition> {
    let mut tools = match router {
        Some(router) => {
            let (user, tasks, recent) = route_inputs(ledger);
            let mut picked: Vec<ToolDefinition> = router
                .select(
                    &RouteContext {
                        last_user_message: &user,
                        task_list: &tasks,
                        recent_tools: &recent,
                        unlocked,
                    },
                    limit,
                )
                .iter()
                .map(shorten_definition)
                .collect();
            if router.hides_tools(limit) {
                picked.push(router.more_tools_definition());
            }
            picked
        }
        None => config.tools.clone(),
    };
    if !tools.is_empty() && ledger_mentions_refs(ledger) {
        tools.push(fetch_output_definition());
    }
    tools
}

fn drive(
    config: &RunConfig,
    ledger: &mut Ledger,
    cancel: &CancelToken,
    sink: &EventSink,
) -> Result<ChatResponse, String> {
    let mut iteration = 0u32;
    let safety = Arc::new(SafetyState::new(&config.safety, ledger, &config.workspace_root));

    let profile = config.profile;
    let token_mode = profile.context_tokens > 0;
    let text_mode = profile.tool_calling == ToolCalling::None && !config.tools.is_empty();
    let router = (profile.is_lite() && !config.tools.is_empty()).then(|| ToolRouter::new(&config.tools));
    let domain_limit = config
        .router_limit
        .unwrap_or_else(|| ToolRouter::limit_for(profile.effective_context()));
    let budget = Budget::for_profile(&profile);
    let unit_estimator = TokenEstimator::for_profile(&profile);
    let mut calibrator = Calibrator::default();
    let mut unlocked: Vec<Category> = Vec::new();
    let mut unparsable_streak = 0u32;
    let store = OutputStore::global();
    // Tool results are cut to a share of the window; characters per token
    // are pessimistic for JSON-heavy output.
    let cap_chars = token_mode.then(|| budget.tool_output_cap_tokens(profile.is_lite()) * 5 / 2);

    loop {
        if cancel.is_cancelled() {
            return Err(CANCELLED_MESSAGE.to_string());
        }

        if iteration >= config.max_iterations {
            let max = config.max_iterations;
            tracing::error!("[agent_chat] max iterations ({max}) reached");
            return Err(format!("Reached the {max}-iteration limit."));
        }
        iteration += 1;

        if let Some(hook) = &config.pre_request_hook {
            hook(ledger);
        }

        let turn_tools = plan_tools(config, ledger, router.as_ref(), &unlocked, domain_limit);
        let protocol = text_mode.then(|| text_tools::protocol_prompt(&turn_tools));
        let estimator = unit_estimator.with_scale(calibrator.scale());

        let compaction = if token_mode {
            let overhead = match &protocol {
                Some(prompt) => estimator.text(prompt),
                None => estimator.tools(&turn_tools),
            };
            let max_tokens = budget.message_tokens(overhead);
            compact_ledger_tokens(ledger, &profile, &budget, max_tokens, &estimator)
        } else {
            compact_ledger(
                ledger,
                &profile,
                config.context_budget_chars,
                config.summary_char_budget,
            )
        };
        if let Some(compaction) = compaction {
            let summary_cap = if token_mode {
                budget.summary_tokens * 3
            } else {
                config.summary_char_budget
            };
            let summary = summarize_into_ledger(
                ledger,
                &compaction,
                &config.provider,
                &config.compact_model,
                Some(summary_cap),
            );
            sink(RunEvent::ContextCompacted(summary));
        }

        let mut messages = project(ledger, &profile);
        if let Some(prompt) = &protocol {
            text_tools::inject_protocol(&mut messages, prompt);
        }
        let request_tools = if text_mode { Vec::new() } else { turn_tools.clone() };
        let request = ChatRequest {
            model: config.model.clone(),
            enable_tool_calls: !request_tools.is_empty(),
            tools: request_tools,
            messages,
            temperature: config.temperature,
            top_p: config.top_p,
            max_tokens: config.max_tokens,
            response_format: config.response_format.clone(),
        };
        // What the estimator says before calibration, to learn from usage.
        let unit_estimate = unit_estimator.request(&request.messages, &request.tools);

        let mut splitter = ThinkSplitter::default();
        let mut fence_filter = text_mode.then(FenceFilter::new);
        let mut streamed_text = String::new();
        let result = config
            .provider
            .chat_streaming(request, &mut |chunk: String| {
                if cancel.is_cancelled() {
                    return;
                }
                streamed_text.push_str(&chunk);
                match fence_filter.as_mut() {
                    Some(filter) => {
                        let visible = filter.feed(&chunk);
                        if !visible.is_empty() {
                            splitter.feed(&visible, sink);
                        }
                    }
                    None => splitter.feed(&chunk, sink),
                }
            })
            .map_err(|err| err.to_string());
        if let Some(rest) = fence_filter.as_mut().map(FenceFilter::finish) {
            if !rest.is_empty() {
                splitter.feed(&rest, sink);
            }
        }
        splitter.finish(sink);

        let mut response = match result {
            Ok(response) => response,
            Err(err) => {
                tracing::error!("[agent_chat] provider error (iter {iteration}): {err}");
                return Err(err);
            }
        };

        if let Some(usage) = usage_from_raw(&response.raw_response) {
            calibrator.observe(unit_estimate, usage.prompt_tokens);
        }

        if text_mode {
            let text = response
                .assistant_message
                .clone()
                .unwrap_or_else(|| streamed_text.clone());
            let known: Vec<&str> = config.tools.iter().map(|t| t.name.as_str()).collect();
            let parsed = text_tools::parse(&text, &known);
            if !parsed.errors.is_empty() && parsed.calls.is_empty() {
                // Tell the model what was wrong and let it try again, but not forever.
                unparsable_streak += 1;
                ledger.push(LedgerEntry::assistant(parsed.text));
                if unparsable_streak > 2 {
                    return Err(format!(
                        "The model kept writing malformed tool calls: {}",
                        parsed.errors.join("; ")
                    ));
                }
                ledger.push(LedgerEntry::engine_notice(text_tools::format_errors(
                    &parsed.errors,
                )));
                continue;
            }
            unparsable_streak = 0;
            response.tool_calls = parsed.calls;
            response.assistant_message = Some(parsed.text);
        }

        if response.tool_calls.is_empty() {
            let text = response
                .assistant_message
                .clone()
                .unwrap_or_else(|| streamed_text.clone());
            ledger.push(LedgerEntry::assistant(text));
            return Ok(response);
        }

        // Tools can have side effects; a cancelled run must not start them.
        if cancel.is_cancelled() {
            return Err(CANCELLED_MESSAGE.to_string());
        }

        // The assistant turn goes in even when empty so the results can follow.
        let assistant_text = response.assistant_message.clone().unwrap_or_default();
        ledger.push(LedgerEntry::assistant(assistant_text.clone()));

        // Text-protocol replies were already streamed (minus the call blocks).
        if !assistant_text.is_empty() && !text_mode {
            sink(RunEvent::Chunk(assistant_text));
        }
        sink(RunEvent::ToolCallGroup(response.tool_calls.clone()));

        let tool_context = make_context(config, sink);
        let meta = MetaTools {
            router: router.as_ref(),
            store,
            cap_chars,
        };
        for (call, result) in run_tools(
            config,
            &safety,
            cancel,
            &response.tool_calls,
            &tool_context,
            sink,
            &meta,
            &mut unlocked,
        ) {
            ledger.push(LedgerEntry::tool_exchange(call, result));
        }
    }
}

fn make_context(config: &RunConfig, sink: &EventSink) -> ToolContext {
    let mut extras = config.tool_extras.clone();

    extras.open_file_request = Some(Arc::new({
        let sink = sink.clone();
        move |path: PathBuf| {
            let reply = UiReply::new();
            sink(RunEvent::OpenFile(path, reply.clone()));
            reply.wait()
        }
    }));

    extras.activate_open_editor_request = Some(Arc::new({
        let sink = sink.clone();
        let query = config.tool_extras.query_open_editors.clone();
        move |index: usize| {
            if let Some(query) = &query {
                let before = query()?;
                let open_count = before
                    .get("open_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                if index >= open_count {
                    let active_before = before
                        .get("active_index")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as usize);
                    return Err(format!(
                        "ActivateOpenEditor index {} is out of range (open_count={}, active_before={:?}).",
                        index, open_count, active_before
                    ));
                }
            }
            let reply = UiReply::new();
            sink(RunEvent::ActivateOpenEditor(index, reply.clone()));
            reply.wait()
        }
    }));

    agent_chat_tools::make_tool_context(config.workspace_root.clone(), None, extras)
}

/// One thread per call so they run concurrently; results come back in call
/// order for message threading.
fn run_tools(
    config: &RunConfig,
    safety: &Arc<SafetyState>,
    cancel: &CancelToken,
    calls: &[ToolCall],
    context: &ToolContext,
    sink: &EventSink,
    meta: &MetaTools<'_>,
    unlocked: &mut Vec<Category>,
) -> Vec<(ToolCall, ToolResult)> {
    enum Slot {
        Done(ToolResult),
        Running(JoinHandle<ToolResult>),
    }

    let parallel = config.profile.parallel_tools;
    let slots: Vec<Slot> = calls
        .iter()
        .map(|call| {
            // Meta tools are answered by the runtime itself.
            if let Some((content, is_error)) = meta.answer(call, unlocked) {
                return Slot::Done(finish_call(call, content, is_error, sink, None, meta.store));
            }
            let call = call.clone();
            let registry = config.tool_registry.clone();
            let ctx = context.clone();
            let sink = sink.clone();
            let safety = safety.clone();
            let cancel = cancel.clone();
            let batch_tainted = safety.tainted_for(&call, calls);
            let workspace_root = config.workspace_root.clone();
            let cap_chars = meta.cap_chars;
            let store = meta.store;
            let handle = std::thread::spawn(move || {
                let result = match safety.preflight(&call, batch_tainted, &cancel, &sink) {
                    Ok(()) => {
                        let result =
                            registry.execute(&call.name, call.arguments_json.clone(), &ctx);
                        safety.observe(&call, &workspace_root);
                        result.map_err(|err| err.to_string())
                    }
                    Err(refusal) => Err(refusal),
                };
                let (content, is_error) = match result {
                    Ok(value) => (value.to_string(), false),
                    Err(err) => (format!("Tool error: {}", err), true),
                };
                finish_call(&call, content, is_error, &sink, cap_chars, store)
            });
            if parallel {
                Slot::Running(handle)
            } else {
                // Weak tool callers get one call at a time.
                Slot::Done(join_tool(handle))
            }
        })
        .collect();

    calls
        .iter()
        .zip(slots)
        .map(|(call, slot)| {
            let result = match slot {
                Slot::Done(result) => result,
                Slot::Running(handle) => join_tool(handle),
            };
            (call.clone(), result)
        })
        .collect()
}

fn join_tool(handle: JoinHandle<ToolResult>) -> ToolResult {
    handle.join().unwrap_or_else(|_| ToolResult {
        content: "Tool thread panicked".to_string(),
        is_error: true,
    })
}

/// Announce a finished call to the UI (uncut) and return what goes into the
/// ledger (cut to `cap_chars`, with a ref to the rest).
fn finish_call(
    call: &ToolCall,
    content: String,
    is_error: bool,
    sink: &EventSink,
    cap_chars: Option<usize>,
    store: &OutputStore,
) -> ToolResult {
    sink(RunEvent::ToolCallResult {
        id: call.id.clone(),
        tool_name: call.name.clone(),
        result_preview: format_tool_result_preview(&call.name, &content),
        result_full: content.clone(),
        is_error,
    });
    let content = match cap_chars {
        Some(max) => cap_output(&content, max, store).text,
        None => content,
    };
    ToolResult { content, is_error }
}

/// The runtime's own tools: `more_tools` and `fetch_output`.
struct MetaTools<'a> {
    router: Option<&'a ToolRouter>,
    store: &'static OutputStore,
    /// Per-result cap in characters; `None` while the window is unknown.
    cap_chars: Option<usize>,
}

impl MetaTools<'_> {
    /// `Some((content, is_error))` when `call` is a meta tool.
    fn answer(&self, call: &ToolCall, unlocked: &mut Vec<Category>) -> Option<(String, bool)> {
        match call.name.as_str() {
            MORE_TOOLS => {
                let Some(router) = self.router else {
                    return Some(("All tools are already available.".to_string(), false));
                };
                let category = call
                    .arguments_json
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Some(match router.unlock(category) {
                    Ok((category, text)) => {
                        if !unlocked.contains(&category) {
                            unlocked.push(category);
                        }
                        (text, false)
                    }
                    Err(err) => (format!("Tool error: {err}"), true),
                })
            }
            FETCH_OUTPUT => {
                let window = self.cap_chars.unwrap_or(8_000);
                Some(match run_fetch_output(&call.arguments_json, self.store, window) {
                    Ok(text) => (text, false),
                    Err(err) => (format!("Tool error: {err}"), true),
                })
            }
            _ => None,
        }
    }
}

/// Splits `<think>…</think>` out of the text stream and batches both streams
/// into UI-sized events.
pub struct ThinkSplitter {
    pending_chunk: String,
    pending_thinking: String,
    last_emit: Instant,
    last_think_emit: Instant,
    in_thinking: bool,
}

impl Default for ThinkSplitter {
    fn default() -> Self {
        Self {
            pending_chunk: String::new(),
            pending_thinking: String::new(),
            last_emit: Instant::now(),
            last_think_emit: Instant::now(),
            in_thinking: false,
        }
    }
}

impl ThinkSplitter {
    pub fn feed(&mut self, chunk: &str, sink: &EventSink) {
        let mut rest: &str = chunk;
        loop {
            if self.in_thinking {
                if let Some(end) = rest.find("</think>") {
                    self.pending_thinking.push_str(&rest[..end]);
                    if !self.pending_thinking.is_empty() {
                        sink(RunEvent::ThinkingChunk(std::mem::take(
                            &mut self.pending_thinking,
                        )));
                    }
                    sink(RunEvent::ThinkingDone);
                    self.in_thinking = false;
                    rest = &rest[end + "</think>".len()..];
                    if rest.is_empty() {
                        break;
                    }
                } else {
                    self.pending_thinking.push_str(rest);
                    // Emit thinking in small batches so the block fills progressively.
                    let should_emit = self.pending_thinking.len() >= 128
                        || self.last_think_emit.elapsed() >= Duration::from_millis(30);
                    if should_emit {
                        sink(RunEvent::ThinkingChunk(std::mem::take(
                            &mut self.pending_thinking,
                        )));
                        self.last_think_emit = Instant::now();
                    }
                    break;
                }
            } else if let Some(start) = rest.find("<think>") {
                let before = &rest[..start];
                if !before.is_empty() {
                    self.pending_chunk.push_str(before);
                    sink(RunEvent::Chunk(std::mem::take(&mut self.pending_chunk)));
                    self.last_emit = Instant::now();
                }
                sink(RunEvent::ThinkingStarted);
                self.in_thinking = true;
                rest = &rest[start + "<think>".len()..];
            } else {
                self.pending_chunk.push_str(rest);
                let should_emit = self.pending_chunk.len() >= 256
                    || self.pending_chunk.contains('\n')
                    || self.last_emit.elapsed() >= Duration::from_millis(24);
                if should_emit {
                    sink(RunEvent::Chunk(std::mem::take(&mut self.pending_chunk)));
                    self.last_emit = Instant::now();
                }
                break;
            }
        }
    }

    /// Flush what is buffered; closes a thinking block the stream left open.
    pub fn finish(&mut self, sink: &EventSink) {
        if !self.pending_chunk.is_empty() {
            sink(RunEvent::Chunk(std::mem::take(&mut self.pending_chunk)));
        }
        if self.in_thinking {
            if !self.pending_thinking.is_empty() {
                sink(RunEvent::ThinkingChunk(std::mem::take(
                    &mut self.pending_thinking,
                )));
            }
            sink(RunEvent::ThinkingDone);
            self.in_thinking = false;
        }
    }
}
