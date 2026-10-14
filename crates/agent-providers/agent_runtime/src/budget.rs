//! Token budgeting. Counts come from the provider when it returns them
//! ([`usage_from_raw`]); otherwise a tokenizer-hint estimator stands in and
//! is calibrated against the provider's counts as they arrive.

use agent_chat_core::{ChatMessage, ToolDefinition};
use serde_json::Value;

use crate::profile::{ModelProfile, TokenizerHint};

/// Fixed per-message cost of role markers and separators.
const MESSAGE_OVERHEAD_TOKENS: usize = 4;
/// Extra cost of one structured tool call (ids, wrappers).
const TOOL_CALL_OVERHEAD_TOKENS: usize = 8;
/// Share of the window kept back for estimator error.
const SAFETY_DIVISOR: usize = 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

/// Pull token counts out of a provider's raw response. Understands the shapes
/// the bundled providers return (OpenAI-style, Anthropic, Gemini, Ollama),
/// both as one object and as an array of stream events.
pub fn usage_from_raw(raw: &Value) -> Option<TokenUsage> {
    let mut prompt: Option<u32> = None;
    let mut completion: Option<u32> = None;

    let mut visit = |event: &Value| {
        let usage = event
            .get("usage")
            .or_else(|| event.get("message").and_then(|m| m.get("usage")))
            .or_else(|| event.get("usageMetadata"));
        if let Some(usage) = usage {
            let get = |keys: &[&str]| {
                keys.iter()
                    .find_map(|key| usage.get(*key).and_then(Value::as_u64))
                    .map(|v| v as u32)
            };
            if let Some(v) = get(&["prompt_tokens", "input_tokens", "promptTokenCount"]) {
                prompt = Some(prompt.map_or(v, |p| p.max(v)));
            }
            if let Some(v) = get(&[
                "completion_tokens",
                "output_tokens",
                "candidatesTokenCount",
            ]) {
                completion = Some(v);
            }
        }
        // Ollama reports on the final event, at the top level.
        if let Some(v) = event.get("prompt_eval_count").and_then(Value::as_u64) {
            prompt = Some(v as u32);
        }
        if let Some(v) = event.get("eval_count").and_then(Value::as_u64) {
            completion = Some(v as u32);
        }
    };

    match raw {
        Value::Array(events) => events.iter().for_each(&mut visit),
        other => visit(other),
    }

    prompt.map(|prompt_tokens| TokenUsage {
        prompt_tokens,
        completion_tokens: completion.unwrap_or(0),
    })
}

/// Estimates tokens for a tokenizer family, optionally scaled by what the
/// provider has actually reported.
#[derive(Clone, Copy, Debug)]
pub struct TokenEstimator {
    hint: TokenizerHint,
    scale: f32,
}

impl TokenEstimator {
    pub fn new(hint: TokenizerHint) -> Self {
        Self { hint, scale: 1.0 }
    }

    pub fn for_profile(profile: &ModelProfile) -> Self {
        Self::new(profile.tokenizer_hint)
    }

    pub fn with_scale(mut self, scale: f32) -> Self {
        self.scale = scale;
        self
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Tokens in `text`. Prose costs `1 / chars_per_token` per character;
    /// digits and punctuation (JSON, code) split into more tokens, and
    /// non-ASCII text is charged a full token per character.
    pub fn text(&self, text: &str) -> usize {
        let per_char = 1.0 / self.hint.chars_per_token();
        let mut cost = 0.0f32;
        for ch in text.chars() {
            cost += if !ch.is_ascii() {
                1.0
            } else if ch.is_ascii_alphabetic() || ch == ' ' || ch == '\n' {
                per_char
            } else {
                0.5
            };
        }
        (cost * self.scale).ceil() as usize
    }

    pub fn message(&self, message: &ChatMessage) -> usize {
        let calls: usize = message
            .tool_calls
            .iter()
            .map(|call| {
                TOOL_CALL_OVERHEAD_TOKENS
                    + self.text(&call.name)
                    + self.text(&call.arguments_json.to_string())
            })
            .sum();
        MESSAGE_OVERHEAD_TOKENS + self.text(&message.content) + calls
    }

    pub fn messages(&self, messages: &[ChatMessage]) -> usize {
        messages.iter().map(|m| self.message(m)).sum()
    }

    pub fn tool(&self, tool: &ToolDefinition) -> usize {
        8 + self.text(&tool.name)
            + tool.description.as_deref().map_or(0, |d| self.text(d))
            + self.text(&tool.parameters_json_schema.to_string())
    }

    pub fn tools(&self, tools: &[ToolDefinition]) -> usize {
        tools.iter().map(|t| self.tool(t)).sum()
    }

    /// A whole request: messages plus tool schemas.
    pub fn request(&self, messages: &[ChatMessage], tools: &[ToolDefinition]) -> usize {
        self.messages(messages) + self.tools(tools)
    }
}

/// Learns how far the estimator is off from the provider's own counts.
#[derive(Clone, Copy, Debug)]
pub struct Calibrator {
    scale: f32,
}

impl Default for Calibrator {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}

impl Calibrator {
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// `estimated` was computed at scale 1.0 for the same request that the
    /// provider reported `actual` prompt tokens for.
    pub fn observe(&mut self, estimated_at_unit_scale: usize, actual_prompt_tokens: u32) {
        if estimated_at_unit_scale == 0 || actual_prompt_tokens == 0 {
            return;
        }
        let ratio = actual_prompt_tokens as f32 / estimated_at_unit_scale as f32;
        // Leaning on the latest sample keeps up with a changing mix of
        // prose and code. Never trust the estimator more than 30% below its
        // raw value; under-counting is what overflows the window.
        self.scale = (0.5 * self.scale + 0.5 * ratio).clamp(0.7, 3.0);
    }
}

/// Token allowances for one request, derived from a profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub context_tokens: usize,
    /// Held back for the model's reply.
    pub reserve_output: usize,
    /// Held back for the compaction summary entry.
    pub summary_tokens: usize,
}

impl Budget {
    pub fn for_profile(profile: &ModelProfile) -> Self {
        let context = profile.effective_context() as usize;
        Self {
            context_tokens: context,
            reserve_output: (profile.max_tokens as usize).min(context / 2),
            summary_tokens: (context / 10).clamp(120, 600),
        }
    }

    /// Tokens the projected messages may use, given what the request carries
    /// besides them (tool schemas, protocol prompts).
    pub fn message_tokens(&self, fixed_overhead: usize) -> usize {
        let usable = self.context_tokens - self.context_tokens / SAFETY_DIVISOR;
        usable
            .saturating_sub(self.reserve_output)
            .saturating_sub(fixed_overhead)
            // Never starve the conversation entirely.
            .max(self.context_tokens / 4)
    }

    /// Smallest dialog allowance compaction may shrink to.
    pub fn dialog_floor(&self) -> usize {
        (self.context_tokens / 10).clamp(200, 1_500)
    }

    /// Cap on one tool result's size, in tokens.
    pub fn tool_output_cap_tokens(&self, lite: bool) -> usize {
        let share = if lite {
            self.context_tokens / 8
        } else {
            self.context_tokens / 10
        };
        share.clamp(256, 16_000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_openai_usage() {
        let raw = json!({"usage": {"prompt_tokens": 120, "completion_tokens": 30}});
        assert_eq!(
            usage_from_raw(&raw),
            Some(TokenUsage {
                prompt_tokens: 120,
                completion_tokens: 30
            })
        );
    }

    #[test]
    fn reads_usage_from_the_last_stream_events() {
        let raw = json!([
            {"choices": [{"delta": {"content": "hi"}}]},
            {"choices": [], "usage": {"prompt_tokens": 77, "completion_tokens": 5}}
        ]);
        assert_eq!(usage_from_raw(&raw).unwrap().prompt_tokens, 77);
    }

    #[test]
    fn reads_anthropic_stream_usage() {
        let raw = json!([
            {"type": "message_start", "message": {"usage": {"input_tokens": 200, "output_tokens": 1}}},
            {"type": "message_delta", "usage": {"output_tokens": 42}}
        ]);
        let usage = usage_from_raw(&raw).unwrap();
        assert_eq!((usage.prompt_tokens, usage.completion_tokens), (200, 42));
    }

    #[test]
    fn reads_ollama_and_gemini_usage() {
        let ollama = json!([{"done": false}, {"done": true, "prompt_eval_count": 55, "eval_count": 9}]);
        assert_eq!(usage_from_raw(&ollama).unwrap().prompt_tokens, 55);
        let gemini = json!({"usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 3}});
        assert_eq!(usage_from_raw(&gemini).unwrap().completion_tokens, 3);
        assert_eq!(usage_from_raw(&json!(null)), None);
    }

    #[test]
    fn estimator_charges_symbols_and_unicode_more_than_prose() {
        let est = TokenEstimator::new(TokenizerHint::Generic);
        let prose = est.text(&"abcd ".repeat(100));
        let json = est.text(&"{\"a\":1}".repeat(100));
        let cjk = est.text(&"漢".repeat(100));
        assert!(prose < json, "{prose} {json}");
        assert!(cjk >= 100);
        assert_eq!(est.text(""), 0);
    }

    #[test]
    fn tool_call_arguments_are_counted() {
        let est = TokenEstimator::new(TokenizerHint::Tiktoken);
        let bare = ChatMessage {
            role: agent_chat_core::ChatRole::Assistant,
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![],
        };
        let mut with_call = bare.clone();
        with_call.tool_calls.push(agent_chat_core::ToolCall {
            id: "1".into(),
            name: "read_file".into(),
            arguments_json: json!({"path": "src/main.rs"}),
        });
        assert!(est.message(&with_call) > est.message(&bare) + 10);
    }

    #[test]
    fn calibrator_follows_the_provider() {
        let mut calibrator = Calibrator::default();
        calibrator.observe(1_000, 1_800);
        assert!(calibrator.scale() > 1.3);
        for _ in 0..8 {
            calibrator.observe(1_000, 1_800);
        }
        assert!((calibrator.scale() - 1.8).abs() < 0.05);
        // Never drops far below the raw estimate.
        for _ in 0..20 {
            calibrator.observe(1_000, 100);
        }
        assert_eq!(calibrator.scale(), 0.7);
    }

    #[test]
    fn budget_leaves_room_for_output_and_tools() {
        let profile = ModelProfile::new(4_096, true);
        let budget = Budget::for_profile(&profile);
        assert_eq!(budget.reserve_output, 1_024);
        let available = budget.message_tokens(300);
        assert!(available + 1_024 + 300 < 4_096);
        assert!(available > 1_500);
        // Overhead larger than the window still leaves a floor.
        assert_eq!(budget.message_tokens(10_000), 1_024);
    }
}
