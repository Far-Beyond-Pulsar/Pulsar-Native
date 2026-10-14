use agent_chat_core::{ChatMessage, ChatProvider, ChatRequest, ChatRole, ToolDefinition};
use agent_chat_tools::{
    PulsarToolExtras, SubagentExecutorRequest, SubagentLlmRequest, SubagentLlmResponse,
    ToolRegistry,
};
use std::sync::Arc;

/// The closure the `spawn_subagent` tool calls to run a sub-agent to completion.
/// Phase 2 replaces this with child runs on `agent_runtime::run_loop`.
pub(super) fn make_subagent_executor(
    registry: ToolRegistry,
    provider: Arc<dyn ChatProvider>,
    default_model: String,
) -> SubagentExecutorRequest {
    Arc::new(move |req: SubagentLlmRequest| {
        let model_used = req
            .model
            .clone()
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| default_model.clone());

        let instructions = req.instructions.unwrap_or_default();

        // Build tool list for sub-agent, excluding UI-only and
        // recursive tools so sub-agents stay focused and safe.
        let sub_tools: Vec<ToolDefinition> = registry
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
                Some(ToolDefinition {
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
            PulsarToolExtras {
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
                response_format: None,
            };

            let mut iter_chunks: Vec<String> = Vec::new();
            let response = provider
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
                };
                serde_json::json!({
                    "role": role,
                    "content": m.content,
                })
            })
            .collect();

        Ok(SubagentLlmResponse {
            provider_id: provider.id().to_string(),
            model_used,
            assistant_message,
            streamed_chunks: all_chunks,
            raw_response: last_raw_response,
            child_transcript,
        })
    })
}
