//! Reading Anthropic's server-sent event stream into a [`ChatResponse`].

use agent_chat_core::{ChatResponse, ToolCall};
use anyhow::{anyhow, Context};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};

use super::MAX_STREAM_BLOCKS;

#[derive(Default)]
struct ToolBlock {
    id: String,
    name: String,
    input_json: String,
}

/// What the stream has produced so far.
#[derive(Default)]
struct StreamState {
    raw_events: Vec<Value>,
    streamed_text_chunks: Vec<String>,
    assistant_message: String,
    finish_reason: Option<String>,
    tool_blocks: Vec<Option<ToolBlock>>,
    thinking_open: bool,
}

pub(super) fn read_stream_response(
    response: reqwest::blocking::Response,
    on_chunk: &mut dyn FnMut(String),
) -> anyhow::Result<ChatResponse> {
    let mut state = StreamState::default();

    let reader = BufReader::new(response);
    for line in reader.lines() {
        let line = line.context("failed reading Anthropic streaming response line")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();

        let event: Value =
            serde_json::from_str(data).context("invalid JSON event in Anthropic stream")?;

        state.apply_event(&event, on_chunk)?;
        state.raw_events.push(event);
    }

    state.close_thinking(on_chunk);
    Ok(state.into_response())
}

impl StreamState {
    fn apply_event(
        &mut self,
        event: &Value,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<()> {
        match event.get("type").and_then(|v| v.as_str()) {
            Some("content_block_start") => self.start_block(event, on_chunk)?,
            Some("content_block_delta") => self.apply_block_delta(event, on_chunk),
            Some("message_delta") => {
                if self.finish_reason.is_none() {
                    self.finish_reason = event
                        .get("delta")
                        .and_then(|d| d.get("stop_reason"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                }
            }
            Some("message_stop") => {
                if self.finish_reason.is_none() {
                    self.finish_reason = Some("stop".to_string());
                }
            }
            Some("error") => {
                let msg = event
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(anyhow!("Anthropic streaming API error: {msg}"));
            }
            _ => {}
        }
        Ok(())
    }

    /// Ends the open `<think>` section, if any.
    fn close_thinking(&mut self, on_chunk: &mut dyn FnMut(String)) {
        if self.thinking_open {
            on_chunk("</think>".to_string());
            self.thinking_open = false;
        }
    }

    fn reserve_block(&mut self, index: usize) {
        while self.tool_blocks.len() <= index {
            self.tool_blocks.push(None);
        }
    }

    fn start_block(
        &mut self,
        event: &Value,
        on_chunk: &mut dyn FnMut(String),
    ) -> anyhow::Result<()> {
        let index = event.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        if index >= MAX_STREAM_BLOCKS {
            return Err(anyhow!(
                "Anthropic stream content block index {index} is out of range"
            ));
        }
        let block = event.get("content_block");
        let block_str = |key: &str| {
            block
                .and_then(|b| b.get(key))
                .and_then(|v| v.as_str())
                .unwrap_or("")
        };

        match block_str("type") {
            "text" => {
                self.close_thinking(on_chunk);
                self.reserve_block(index);
            }
            "thinking" => self.reserve_block(index),
            "tool_use" => {
                self.close_thinking(on_chunk);
                let id = block_str("id").to_string();
                let name = block_str("name").to_string();
                self.reserve_block(index);
                self.tool_blocks[index] = Some(ToolBlock {
                    id,
                    name,
                    input_json: String::new(),
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_block_delta(&mut self, event: &Value, on_chunk: &mut dyn FnMut(String)) {
        let index = event.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let Some(delta) = event.get("delta") else {
            return;
        };
        match delta.get("type").and_then(|v| v.as_str()) {
            Some("thinking_delta") => {
                if let Some(thinking) = delta.get("thinking").and_then(|v| v.as_str()) {
                    self.push_thinking(thinking, on_chunk);
                }
            }
            Some("text_delta") => {
                if let Some(text) = delta.get("text").and_then(|v| v.as_str()) {
                    self.push_text(text, on_chunk);
                }
            }
            Some("input_json_delta") => {
                if let Some(partial) = delta.get("partial_json").and_then(|v| v.as_str()) {
                    if let Some(Some(block)) = self.tool_blocks.get_mut(index) {
                        block.input_json.push_str(partial);
                    }
                }
            }
            _ => {}
        }
    }

    fn push_thinking(&mut self, thinking: &str, on_chunk: &mut dyn FnMut(String)) {
        if thinking.is_empty() {
            return;
        }
        if !self.thinking_open {
            on_chunk(format!("<think>{}", thinking));
            self.thinking_open = true;
        } else {
            on_chunk(thinking.to_string());
        }
    }

    fn push_text(&mut self, text: &str, on_chunk: &mut dyn FnMut(String)) {
        if text.is_empty() {
            return;
        }
        self.close_thinking(on_chunk);
        let chunk = text.to_string();
        self.assistant_message.push_str(&chunk);
        self.streamed_text_chunks.push(chunk.clone());
        on_chunk(chunk);
    }

    fn into_response(self) -> ChatResponse {
        tracing::debug!(
            "[agent_provider_anthropic] streaming completed chunks={} assistant_len={} \
             finish_reason={}",
            self.streamed_text_chunks.len(),
            self.assistant_message.len(),
            self.finish_reason.as_deref().unwrap_or("none"),
        );

        let tool_calls: Vec<ToolCall> = self
            .tool_blocks
            .into_iter()
            .flatten()
            .filter(|b| !b.name.is_empty())
            .map(|b| {
                let arguments_json = if b.input_json.trim().is_empty() {
                    json!({})
                } else {
                    serde_json::from_str::<Value>(&b.input_json)
                        .unwrap_or_else(|_| Value::String(b.input_json.clone()))
                };
                ToolCall {
                    id: b.id,
                    name: b.name,
                    arguments_json,
                }
            })
            .collect();

        ChatResponse {
            assistant_message: if self.assistant_message.is_empty() {
                None
            } else {
                Some(self.assistant_message)
            },
            streamed_text_chunks: self.streamed_text_chunks,
            tool_calls,
            finish_reason: self.finish_reason,
            raw_response: Value::Array(self.raw_events),
        }
    }
}
