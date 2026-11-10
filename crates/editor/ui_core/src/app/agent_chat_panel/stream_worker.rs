//! The agentic loop that runs on the provider worker thread.
//!
//! Owns [`WorkerContext`] (everything the thread needs, passed explicitly
//! instead of captured by a giant closure) and [`run_agent_loop`]: up to
//! [`MAX_ITERATIONS`] rounds of "stream a response, run any tool calls,
//! compact if over budget". Events go to the UI through the channel in the
//! context; the terminal `Finished` event is sent exactly once via
//! [`send_finished_once`], shared with the watchdog timer in `streaming.rs`.
//! Entry point: [`run_agent_loop`], called inside `std::thread::spawn`.

use super::request_prep::insert_compaction_summary;
use super::stream_event::StreamEvent;
use super::stream_tools::{execute_tool_calls, make_agent_tool_context};
use super::think_parser::{ThinkEvent, ThinkParser};
use super::*;
use agent_chat_core::{ChatRequest, ChatResponse};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// Hard cap on provider round-trips for a single user prompt.
pub(super) const MAX_ITERATIONS: u32 = 50;

impl From<ThinkEvent> for StreamEvent {
    fn from(event: ThinkEvent) -> Self {
        match event {
            ThinkEvent::Chunk(text) => StreamEvent::Chunk(text),
            ThinkEvent::ThinkingStarted => StreamEvent::ThinkingStarted,
            ThinkEvent::ThinkingChunk(text) => StreamEvent::ThinkingChunk(text),
            ThinkEvent::ThinkingDone => StreamEvent::ThinkingDone,
        }
    }
}

/// Everything the worker thread needs; moved into the thread as one value.
pub(super) struct WorkerContext {
    pub(super) provider: Arc<dyn ChatProvider>,
    /// The first request; later iterations reuse its model, tools and sampling.
    pub(super) request: ChatRequest,
    pub(super) token: String,
    pub(super) compact_model: String,
    /// Character budget that triggers in-loop compaction.
    pub(super) context_budget: usize,
    pub(super) tool_registry: ToolRegistry,
    pub(super) plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    pub(super) tx: smol::channel::Sender<StreamEvent>,
    /// The UI sends `()` here to abort the request.
    pub(super) cancel_rx: smol::channel::Receiver<()>,
    /// Set by whoever sends `Finished` first (worker or watchdog).
    pub(super) completion_sent: Arc<AtomicBool>,
}

/// Send `StreamEvent::Finished(result)` unless a `Finished` was already sent.
/// The atomic swap makes this exactly-once across the worker and the watchdog.
/// Returns whether this call sent the event.
pub(super) fn send_finished_once(
    completion_sent: &AtomicBool,
    tx: &smol::channel::Sender<StreamEvent>,
    result: Result<ChatResponse, String>,
) -> bool {
    if !completion_sent.swap(true, Ordering::SeqCst) {
        let _ = tx.try_send(StreamEvent::Finished(result));
        true
    } else {
        false
    }
}

/// Decide whether the loop must stop before starting iteration number
/// `iteration` (0-based count of rounds already started). Returns the error
/// text for `Finished` when it must: cancellation wins over the iteration cap.
pub(super) fn loop_stop_reason(
    cancel_rx: &smol::channel::Receiver<()>,
    iteration: u32,
) -> Option<String> {
    // Check for user cancellation between iterations.
    if cancel_rx.try_recv().is_ok() {
        return Some("Request cancelled.".to_string());
    }

    if iteration >= MAX_ITERATIONS {
        tracing::error!("[agent_chat] max iterations ({MAX_ITERATIONS}) reached");
        // Treat hitting the limit as a clean finish so the UI isn't left hanging.
        return Some(format!("Reached the {MAX_ITERATIONS}-iteration limit."));
    }
    None
}

/// Run the agent loop to completion, then return (ending the thread).
pub(super) fn run_agent_loop(ctx: WorkerContext) {
    let mut current_messages = ctx.request.messages.clone();
    let mut iteration = 0u32;

    loop {
        if let Some(reason) = loop_stop_reason(&ctx.cancel_rx, iteration) {
            send_finished_once(&ctx.completion_sent, &ctx.tx, Err(reason));
            break;
        }
        iteration += 1;

        let current_request = ChatRequest {
            model: ctx.request.model.clone(),
            messages: current_messages.clone(),
            enable_tool_calls: ctx.request.enable_tool_calls,
            tools: ctx.request.tools.clone(),
            temperature: ctx.request.temperature,
            top_p: ctx.request.top_p,
            max_tokens: ctx.request.max_tokens,
        };

        let result = stream_one_response(&ctx, current_request.clone());

        match result {
            Ok(response) => {
                if !response.tool_calls.is_empty() {
                    run_tool_round(
                        &ctx,
                        &mut current_messages,
                        &response,
                        &current_request.model,
                    );
                    compact_if_over_budget(&ctx, &mut current_messages);
                    continue;
                } else {
                    send_finished_once(&ctx.completion_sent, &ctx.tx, Ok(response));
                    break;
                }
            }
            Err(err) => {
                tracing::error!("[agent_chat] provider error (iter {iteration}): {err}");
                send_finished_once(&ctx.completion_sent, &ctx.tx, Err(err));
                break;
            }
        }
    }
}

/// Stream one provider response, forwarding parsed prose/thinking events to
/// the UI as they arrive and flushing buffered text when the stream ends.
fn stream_one_response(ctx: &WorkerContext, request: ChatRequest) -> Result<ChatResponse, String> {
    let mut parser = ThinkParser::new(Instant::now());
    let mut forward = |event: ThinkEvent| {
        let _ = ctx.tx.try_send(event.into());
    };

    let result = ctx
        .provider
        .chat_streaming(request, &mut |chunk: String| {
            parser.push_chunk(&chunk, Instant::now(), &mut forward)
        })
        .map_err(|err| err.to_string());

    parser.finish(&mut forward);
    result
}

/// Handle a response that requested tools: record the assistant turn, tell the
/// UI, run the tools concurrently and append their results to the history.
fn run_tool_round(
    ctx: &WorkerContext,
    current_messages: &mut Vec<ChatMessage>,
    response: &ChatResponse,
    model: &str,
) {
    let assistant_text = response.assistant_message.clone().unwrap_or_default();
    current_messages.push(ChatMessage {
        role: ChatRole::Assistant,
        content: assistant_text.clone(),
        tool_call_id: None,
        tool_calls: response.tool_calls.clone(),
    });

    // Show assistant text to user (if any)
    if !assistant_text.is_empty() {
        let _ = ctx.tx.try_send(StreamEvent::Chunk(assistant_text));
    }

    // Tell the UI to render a collapsed tool-call block.
    let _ = ctx
        .tx
        .try_send(StreamEvent::ToolCallGroup(response.tool_calls.clone()));

    // Create tool context for execution
    let tool_context = make_agent_tool_context(
        &ctx.tx,
        &ctx.tool_registry,
        ctx.plugin_bridge.clone(),
        &ctx.provider,
        model.to_string(),
    );

    let all_results = execute_tool_calls(
        &ctx.tool_registry,
        &tool_context,
        &ctx.tx,
        &response.tool_calls,
    );

    for (tool_call_id, _tool_name, tool_result) in all_results {
        current_messages.push(ChatMessage {
            role: ChatRole::Tool,
            content: tool_result,
            tool_call_id: Some(tool_call_id),
            tool_calls: vec![],
        });
    }
}

/// Compact `current_messages` if tool results pushed us over budget.
fn compact_if_over_budget(ctx: &WorkerContext, current_messages: &mut Vec<ChatMessage>) {
    let total_chars: usize = current_messages.iter().map(|m| m.content.len()).sum();
    if total_chars > ctx.context_budget {
        let (mut compacted, dropped_opt) = super::prompt_ranking::compact_messages(
            std::mem::take(current_messages),
            ctx.context_budget,
            AgentChatPanel::COMPACTION_SUMMARY_CHAR_BUDGET,
        );
        if let Some(dropped) = dropped_opt {
            let summary = AgentChatPanel::ai_summarize_dropped(
                &dropped,
                &ctx.provider,
                &ctx.compact_model,
                &ctx.token,
            );
            insert_compaction_summary(&mut compacted, &summary);
            let _ = ctx.tx.try_send(StreamEvent::ContextCompacted(summary));
        }
        *current_messages = compacted;
    }
}

#[cfg(test)]
mod tests {
    use super::{loop_stop_reason, send_finished_once, StreamEvent, ThinkEvent, MAX_ITERATIONS};
    use std::sync::atomic::AtomicBool;

    fn finished_count(rx: &smol::channel::Receiver<StreamEvent>) -> usize {
        let mut n = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, StreamEvent::Finished(_)) {
                n += 1;
            }
        }
        n
    }

    #[test]
    fn finished_is_sent_exactly_once() {
        let (tx, rx) = smol::channel::unbounded::<StreamEvent>();
        let flag = AtomicBool::new(false);
        assert!(send_finished_once(&flag, &tx, Err("first".into())));
        assert!(!send_finished_once(&flag, &tx, Err("second".into())));
        assert!(!send_finished_once(&flag, &tx, Err("third".into())));
        match rx.try_recv() {
            Ok(StreamEvent::Finished(Err(msg))) => assert_eq!(msg, "first"),
            _ => panic!("expected the first Finished event"),
        }
        assert_eq!(finished_count(&rx), 0);
    }

    #[test]
    fn finished_is_not_sent_if_flag_already_set() {
        let (tx, rx) = smol::channel::unbounded::<StreamEvent>();
        let flag = AtomicBool::new(true);
        assert!(!send_finished_once(&flag, &tx, Err("late".into())));
        assert_eq!(finished_count(&rx), 0);
    }

    #[test]
    fn loop_continues_without_cancel_below_limit() {
        let (_cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
        assert_eq!(loop_stop_reason(&cancel_rx, 0), None);
        assert_eq!(loop_stop_reason(&cancel_rx, MAX_ITERATIONS - 1), None);
    }

    #[test]
    fn loop_stops_at_iteration_limit() {
        let (_cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
        assert_eq!(
            loop_stop_reason(&cancel_rx, MAX_ITERATIONS).as_deref(),
            Some("Reached the 50-iteration limit.")
        );
    }

    #[test]
    fn cancel_is_consumed_and_wins_over_limit() {
        let (cancel_tx, cancel_rx) = smol::channel::bounded::<()>(1);
        cancel_tx.try_send(()).unwrap();
        assert_eq!(
            loop_stop_reason(&cancel_rx, MAX_ITERATIONS).as_deref(),
            Some("Request cancelled.")
        );
        // The signal was consumed, so a later check no longer reports it.
        assert_eq!(loop_stop_reason(&cancel_rx, 0), None);
    }

    #[test]
    fn think_events_map_to_stream_events() {
        assert!(matches!(
            StreamEvent::from(ThinkEvent::Chunk("x".into())),
            StreamEvent::Chunk(t) if t == "x"
        ));
        assert!(matches!(
            StreamEvent::from(ThinkEvent::ThinkingStarted),
            StreamEvent::ThinkingStarted
        ));
        assert!(matches!(
            StreamEvent::from(ThinkEvent::ThinkingChunk("y".into())),
            StreamEvent::ThinkingChunk(t) if t == "y"
        ));
        assert!(matches!(
            StreamEvent::from(ThinkEvent::ThinkingDone),
            StreamEvent::ThinkingDone
        ));
    }
}
