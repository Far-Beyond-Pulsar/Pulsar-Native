//! Tool execution support for the agent loop.
//!
//! Owns building the `ToolContext` handed to tools (including the callbacks
//! that route open-file / activate-editor requests to the UI thread and the
//! sub-agent executor) and running one round of tool calls concurrently.
//! Entry points: [`make_agent_tool_context`] and [`execute_tool_calls`], both
//! called from `stream_worker.rs` on the worker thread.

use super::stream_event::{StreamEvent, UiWaiter};
use super::*;
use agent_chat_core::{ChatRequest, ToolCall};
use std::sync::{Condvar, Mutex};

/// Build the tool context for one tool round. `default_subagent_model` is used
/// by sub-agents that do not request a specific model.
pub(super) fn make_agent_tool_context(
    tx: &smol::channel::Sender<StreamEvent>,
    tool_registry: &ToolRegistry,
    plugin_bridge: Option<Arc<RwLock<plugin_manager::PluginToolBridge>>>,
    provider: &Arc<dyn ChatProvider>,
    default_subagent_model: String,
) -> agent_chat_tools::ToolContext {
    let workspace_root = match engine_state::get_project_path() {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from("."),
    };
    agent_chat_tools::make_tool_context(
        workspace_root,
        None,
        agent_chat_tools::PulsarToolExtras {
            plugin_bridge,
            open_file_request: Some(open_file_request(tx)),
            query_open_editors: Some(Arc::new(|| Ok(crate::app::open_editors::snapshot_json()))),
            activate_open_editor_request: Some(activate_open_editor_request(tx)),
            subagent_executor: Some(subagent_executor(
                tool_registry.clone(),
                provider.clone(),
                default_subagent_model,
            )),
        },
    )
}

/// Callback that asks the UI thread to open a file and blocks until it answers.
fn open_file_request(tx: &smol::channel::Sender<StreamEvent>) -> agent_chat_tools::OpenFileRequest {
    let tx_for_open = tx.clone();
    Arc::new(move |path: PathBuf| {
        let waiter = Arc::new((Mutex::new(None), Condvar::new()));

        tx_for_open
            .try_send(StreamEvent::OpenFile(path, waiter.clone()))
            .map_err(|err| format!("Failed to dispatch open-file request to UI thread: {}", err))?;

        let (lock, cvar) = &*waiter;
        let mut guard = lock
            .lock()
            .map_err(|_| "OpenFile waiter mutex poisoned".to_string())?;
        while guard.is_none() {
            guard = cvar
                .wait(guard)
                .map_err(|_| "OpenFile waiter mutex poisoned".to_string())?;
        }

        guard
            .take()
            .unwrap_or_else(|| Err("OpenFile waiter signaled without a result".to_string()))
    })
}

/// Callback that asks the UI thread to focus an open editor tab and blocks
/// until it answers.
fn activate_open_editor_request(
    tx: &smol::channel::Sender<StreamEvent>,
) -> agent_chat_tools::ActivateOpenEditorRequest {
    let tx_for_activate = tx.clone();
    Arc::new(move |index: usize| {
        let before = crate::app::open_editors::snapshot_json();
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

        let waiter = Arc::new((Mutex::new(None), Condvar::new()));

        tx_for_activate
            .try_send(StreamEvent::ActivateOpenEditor(index, waiter.clone()))
            .map_err(|err| {
                format!(
                    "Failed to dispatch activate-open-editor request to UI thread: {}",
                    err
                )
            })?;

        let (lock, cvar) = &*waiter;
        let mut guard = lock
            .lock()
            .map_err(|_| "ActivateOpenEditor waiter mutex poisoned".to_string())?;
        while guard.is_none() {
            guard = cvar
                .wait(guard)
                .map_err(|_| "ActivateOpenEditor waiter mutex poisoned".to_string())?;
        }

        guard.take().unwrap_or_else(|| {
            Err("ActivateOpenEditor waiter signaled without a result".to_string())
        })
    })
}

/// Executor used by the `spawn_subagent` tool: runs a bounded tool-calling loop
/// against `provider` with a filtered tool set and returns its transcript.
fn subagent_executor(
    registry: ToolRegistry,
    provider_for_subagent: Arc<dyn ChatProvider>,
    default_subagent_model: String,
) -> agent_chat_tools::SubagentExecutorRequest {
    Arc::new(move |req: agent_chat_tools::SubagentLlmRequest| {
        let model_used = req
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| default_subagent_model.clone());

        let instructions = req.instructions.unwrap_or_default();

        // Build tool list for sub-agent, excluding UI-only and
        // recursive tools so sub-agents stay focused and safe.
        let sub_tools: Vec<agent_chat_core::ToolDefinition> = registry
            .definitions()
            .into_iter()
            .filter(|def| {
                !matches!(
                    def.name.as_str(),
                    "spawn_subagent"
                        | "query_running_subagents"
                        | "open_file_in_default_editor"
                        | "activate_open_editor"
                        | "query_open_editors"
                )
            })
            .filter_map(|def| {
                let params = def.parameters_schema;
                if params.get("type").and_then(|v| v.as_str()) != Some("object") {
                    return None;
                }
                Some(agent_chat_core::ToolDefinition {
                    name: def.name,
                    description: Some(def.description),
                    parameters_json_schema: params,
                })
            })
            .collect();

        let enable_tools = !sub_tools.is_empty();

        let sub_tool_context = agent_chat_tools::make_tool_context(
            req.workspace_root.clone(),
            None,
            agent_chat_tools::PulsarToolExtras {
                plugin_bridge: None,
                open_file_request: None,
                query_open_editors: None,
                activate_open_editor_request: None,
                subagent_executor: None,
            },
        );

        let mut current_messages = vec![
                                    ChatMessage {
                                        role: ChatRole::System,
                                        content: format!(
                                            "You are an AI assistant completing a specific task.\n\
                                         \n\
                                         TASK NAME: {name}\n\
                                         \n\
                                         TASK: {task}\n\
                                         \n\
                                         INSTRUCTIONS: {instructions}\n\
                                         \n\
                                         WORKSPACE: {workspace}\n\
                                         \n\
                                         Use the available tools to read files, search the codebase, and gather evidence.\n\
                                         Include file paths, line numbers, and direct quotes in your answer.\n\
                                         When you have enough information, write your final answer and stop calling tools.",
                                            name = req.name,
                                            task = req.task,
                                            instructions = instructions,
                                            workspace = req.workspace_root.display(),
                                        ),
                                        tool_call_id: None,
                                        tool_calls: vec![],
                                    },
                                    ChatMessage {
                                        role: ChatRole::User,
                                        content: "Complete the task.".to_string(),
                                        tool_call_id: None,
                                        tool_calls: vec![],
                                    },
                                ];

        let mut all_chunks: Vec<String> = Vec::new();
        let mut last_raw_response = serde_json::Value::Null;
        const MAX_SUB_ITERATIONS: u32 = 8;

        for _sub_iter in 0..MAX_SUB_ITERATIONS {
            let sub_request = ChatRequest {
                model: model_used.clone(),
                messages: current_messages.clone(),
                enable_tool_calls: enable_tools,
                tools: sub_tools.clone(),
                temperature: Some(0.2),
                top_p: Some(1.0),
                max_tokens: Some(4096),
            };

            let mut iter_chunks: Vec<String> = Vec::new();
            let response = provider_for_subagent
                .chat_streaming(sub_request, &mut |chunk| iter_chunks.push(chunk))
                .map_err(|e| format!("Sub-agent provider error: {e}"))?;

            all_chunks.extend(iter_chunks.clone());
            last_raw_response = response.raw_response.clone();

            let assistant_text = response
                .assistant_message
                .clone()
                .or_else(|| {
                    if iter_chunks.is_empty() {
                        None
                    } else {
                        Some(iter_chunks.join(""))
                    }
                })
                .unwrap_or_default();

            if response.tool_calls.is_empty() {
                current_messages.push(ChatMessage {
                    role: ChatRole::Assistant,
                    content: assistant_text,
                    tool_call_id: None,
                    tool_calls: vec![],
                });
                break;
            }

            // Assistant turn with tool calls
            current_messages.push(ChatMessage {
                role: ChatRole::Assistant,
                content: assistant_text,
                tool_call_id: None,
                tool_calls: response.tool_calls.clone(),
            });

            // Execute tools sequentially (sub-agent context)
            for call in &response.tool_calls {
                let result = registry
                    .execute(&call.name, call.arguments_json.clone(), &sub_tool_context)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|e| format!("Tool error: {e}"));
                current_messages.push(ChatMessage {
                    role: ChatRole::Tool,
                    content: result,
                    tool_call_id: Some(call.id.clone()),
                    tool_calls: vec![],
                });
            }
        }

        // Extract the final assistant response from the transcript
        let assistant_message = current_messages
            .iter()
            .rev()
            .find(|m| m.role == ChatRole::Assistant && !m.content.is_empty())
            .map(|m| m.content.clone())
            .unwrap_or_default();

        let child_transcript = current_messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    ChatRole::System => "system",
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                    ChatRole::Tool => "tool",
                    ChatRole::AgentEvent => "agent_event",
                };
                serde_json::json!({
                    "role": role,
                    "content": m.content,
                })
            })
            .collect();

        Ok(agent_chat_tools::SubagentLlmResponse {
            provider_id: provider_for_subagent.id().to_string(),
            model_used,
            assistant_message,
            streamed_chunks: all_chunks,
            raw_response: last_raw_response,
            child_transcript,
        })
    })
}

/// Execute `tool_calls` on one thread each and return `(id, name, result)` in
/// the original call order. Each result is also sent to the UI as it lands.
pub(super) fn execute_tool_calls(
    tool_registry: &ToolRegistry,
    tool_context: &agent_chat_tools::ToolContext,
    tx: &smol::channel::Sender<StreamEvent>,
    tool_calls: &[ToolCall],
) -> Vec<(String, String, String)> {
    // Spawn one thread per tool call so they execute concurrently.
    let mut all_results = Vec::new();
    let handles: Vec<_> = tool_calls
        .iter()
        .map(|tool_call| {
            let name = tool_call.name.clone();
            let args = tool_call.arguments_json.clone();
            let id = tool_call.id.clone();
            let registry = tool_registry.clone();
            let ctx = tool_context.clone();
            let tx = tx.clone();
            editor_task_queue::spawn_named(
                editor_task_queue::ThreadSpec::new(
                    "ui core: execute tool calls",
                    editor_task_queue::ThreadRole::Io,
                )
                .owner("ui_core"),
                move || {
                    let result = registry.execute(&name, args, &ctx);
                    let (tool_result, is_error) = match result {
                        Ok(value) => (value.to_string(), false),
                        Err(err) => (format!("Tool error: {}", err), true),
                    };
                    let result_preview =
                        AgentChatPanel::format_tool_result_preview(&name, &tool_result);
                    let _ = tx.try_send(StreamEvent::ToolCallResult {
                        id: id.clone(),
                        tool_name: name.clone(),
                        result_preview,
                        result_full: tool_result.clone(),
                        is_error,
                    });
                    (id, name, tool_result)
                },
            )
        })
        .collect();

    // Collect parallel results in original order for message threading.
    for handle in handles {
        all_results.push(handle.join().unwrap_or_else(|_| {
            (
                "".to_string(),
                "unknown".to_string(),
                "Tool thread panicked".to_string(),
            )
        }));
    }
    all_results
}
