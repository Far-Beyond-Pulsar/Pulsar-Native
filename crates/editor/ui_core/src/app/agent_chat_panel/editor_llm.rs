//! Publishes the chat panel's selected provider and model to the editors.
//!
//! Editors do their own one-shot completions (inline edit, explain, quick
//! fix) through [`plugin_editor_api::llm`]; this is the one place that knows
//! which provider the user set up and chose.

use super::*;
use plugin_editor_api::llm::{self, EditorLlm, LlmRequest, LlmRole};

struct PanelLlm {
    provider: Arc<dyn ChatProvider>,
    model: String,
    label: String,
}

impl EditorLlm for PanelLlm {
    fn label(&self) -> String {
        self.label.clone()
    }

    fn complete(
        &self,
        request: LlmRequest,
        on_chunk: &mut dyn FnMut(&str),
    ) -> Result<String, String> {
        let mut messages = Vec::with_capacity(request.messages.len() + 1);
        let message = |role, content: String| ChatMessage {
            role,
            content,
            tool_call_id: None,
            tool_calls: vec![],
        };
        if !request.system.is_empty() {
            messages.push(message(ChatRole::System, request.system));
        }
        for turn in request.messages {
            let role = match turn.role {
                LlmRole::User => ChatRole::User,
                LlmRole::Assistant => ChatRole::Assistant,
            };
            messages.push(message(role, turn.content));
        }
        let chat = agent_chat_core::ChatRequest {
            model: self.model.clone(),
            messages,
            enable_tool_calls: false,
            tools: Vec::new(),
            temperature: request.temperature,
            top_p: None,
            max_tokens: request.max_tokens,
        };
        let mut streamed = String::new();
        let response = self
            .provider
            .chat_streaming(chat, &mut |chunk| {
                on_chunk(&chunk);
                streamed.push_str(&chunk);
            })
            .map_err(|error| error.to_string())?;
        Ok(response.assistant_message.unwrap_or(streamed))
    }
}

impl AgentChatPanel {
    /// Makes the selected provider and model the editors' language model, or
    /// withdraws it while the provider is not ready. Call after anything
    /// that changes the selection or a provider's state.
    pub(super) fn sync_editor_llm(&self) {
        let selected = self
            .active_provider()
            .zip(self.active_model())
            .filter(|(provider, _)| {
                self.provider_states.get(provider.id) == Some(&ProviderState::Ready)
            })
            .and_then(|(provider, model)| {
                let implementation = self.provider_registry.get(provider.id)?.clone();
                Some(PanelLlm {
                    provider: implementation,
                    model: model.id.to_string(),
                    label: format!("{} · {}", model.label, provider.label),
                })
            });
        llm::set_editor_llm(selected.map(|llm| Arc::new(llm) as Arc<dyn EditorLlm>));
    }
}
