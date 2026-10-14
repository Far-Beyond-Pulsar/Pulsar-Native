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

use agent_chat_core::{ChatProvider, ChatRequest, ChatResponse, ToolCall, ToolDefinition};
use agent_chat_tools::{PulsarToolExtras, ToolContext, ToolRegistry};

use crate::compaction::{compact_ledger, summarize_compaction};
use crate::ledger::{Ledger, LedgerEntry, ToolResult};
use crate::preview::format_tool_result_preview;
use crate::profile::ModelProfile;
use crate::project::project;

pub const CANCELLED_MESSAGE: &str = "Request cancelled.";

/// Cooperative cancellation. Cloning shares the flag.
#[derive(Clone, Debug, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
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
    /// Always the last event, exactly once.
    Finished(Result<ChatResponse, String>),
}

pub type EventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;

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
    pub context_budget_chars: usize,
    pub summary_char_budget: usize,
    pub max_iterations: u32,
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
        }
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
    config: RunConfig,
    mut ledger: Ledger,
    cancel: CancelToken,
    sink: EventSink,
) -> Ledger {
    let outcome = drive(&config, &mut ledger, &cancel, &sink);
    sink(RunEvent::Finished(outcome));
    ledger
}

fn drive(
    config: &RunConfig,
    ledger: &mut Ledger,
    cancel: &CancelToken,
    sink: &EventSink,
) -> Result<ChatResponse, String> {
    let mut iteration = 0u32;

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

        if let Some(compaction) = compact_ledger(
            ledger,
            &config.profile,
            config.context_budget_chars,
            config.summary_char_budget,
        ) {
            let summary =
                summarize_compaction(ledger, &compaction, &config.provider, &config.compact_model);
            sink(RunEvent::ContextCompacted(summary));
        }

        let request = ChatRequest {
            model: config.model.clone(),
            messages: project(ledger, &config.profile),
            enable_tool_calls: !config.tools.is_empty(),
            tools: config.tools.clone(),
            temperature: config.temperature,
            top_p: config.top_p,
            max_tokens: config.max_tokens,
        };

        let mut splitter = ThinkSplitter::default();
        let mut streamed_text = String::new();
        let result = config
            .provider
            .chat_streaming(request, &mut |chunk: String| {
                if cancel.is_cancelled() {
                    return;
                }
                streamed_text.push_str(&chunk);
                splitter.feed(&chunk, sink);
            })
            .map_err(|err| err.to_string());
        splitter.finish(sink);

        let response = match result {
            Ok(response) => response,
            Err(err) => {
                tracing::error!("[agent_chat] provider error (iter {iteration}): {err}");
                return Err(err);
            }
        };

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

        if !assistant_text.is_empty() {
            sink(RunEvent::Chunk(assistant_text));
        }
        sink(RunEvent::ToolCallGroup(response.tool_calls.clone()));

        let tool_context = make_context(config, sink);
        for (call, result) in run_tools(config, &response.tool_calls, &tool_context, sink) {
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
    calls: &[ToolCall],
    context: &ToolContext,
    sink: &EventSink,
) -> Vec<(ToolCall, ToolResult)> {
    let handles: Vec<_> = calls
        .iter()
        .map(|call| {
            let call = call.clone();
            let registry = config.tool_registry.clone();
            let ctx = context.clone();
            let sink = sink.clone();
            std::thread::spawn(move || {
                let result = registry.execute(&call.name, call.arguments_json.clone(), &ctx);
                let (content, is_error) = match result {
                    Ok(value) => (value.to_string(), false),
                    Err(err) => (format!("Tool error: {}", err), true),
                };
                sink(RunEvent::ToolCallResult {
                    id: call.id.clone(),
                    tool_name: call.name.clone(),
                    result_preview: format_tool_result_preview(&call.name, &content),
                    result_full: content.clone(),
                    is_error,
                });
                ToolResult { content, is_error }
            })
        })
        .collect();

    calls
        .iter()
        .zip(handles)
        .map(|(call, handle)| {
            let result = handle.join().unwrap_or_else(|_| ToolResult {
                content: "Tool thread panicked".to_string(),
                is_error: true,
            });
            (call.clone(), result)
        })
        .collect()
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
