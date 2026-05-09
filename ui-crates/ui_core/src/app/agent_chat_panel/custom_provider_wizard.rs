use super::*;
use crate::custom_providers::{self, CustomProvider};
use agent_provider_openai::OpenAiCompatibleProvider;
use std::path::PathBuf;

impl AgentChatPanel {
    pub(super) fn custom_provider_config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("pulsar")
    }

    pub(super) fn add_provider_prompt_title(step: AddProviderPromptStep) -> &'static str {
        match step {
            AddProviderPromptStep::ProviderId => "Provider ID (example: my_local_provider)",
            AddProviderPromptStep::ProviderLabel => "Provider label (example: My Local Provider)",
            AddProviderPromptStep::Endpoint => "Provider endpoint URL",
            AddProviderPromptStep::ModelId => "Default model ID",
            AddProviderPromptStep::ModelLabel => "Default model label",
            AddProviderPromptStep::ModelSupportsTools => "Does this model support tools? (yes/no)",
        }
    }

    pub(super) fn next_add_provider_step(
        step: AddProviderPromptStep,
    ) -> Option<AddProviderPromptStep> {
        match step {
            AddProviderPromptStep::ProviderId => Some(AddProviderPromptStep::ProviderLabel),
            AddProviderPromptStep::ProviderLabel => Some(AddProviderPromptStep::Endpoint),
            AddProviderPromptStep::Endpoint => Some(AddProviderPromptStep::ModelId),
            AddProviderPromptStep::ModelId => Some(AddProviderPromptStep::ModelLabel),
            AddProviderPromptStep::ModelLabel => Some(AddProviderPromptStep::ModelSupportsTools),
            AddProviderPromptStep::ModelSupportsTools => None,
        }
    }

    pub(super) fn start_add_provider_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_custom_provider = Some(PendingCustomProvider::default());
        self.pending_custom_provider_step = Some(AddProviderPromptStep::ProviderId);
        self.custom_provider_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        cx.notify();
    }

    pub(super) fn cancel_add_provider_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_custom_provider = None;
        self.pending_custom_provider_step = None;
        self.custom_provider_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        cx.notify();
    }

    pub(super) fn submit_add_provider_prompt(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(step) = self.pending_custom_provider_step else {
            return;
        };
        let Some(pending) = self.pending_custom_provider.as_mut() else {
            return;
        };

        let value = self.custom_provider_input.read(cx).text().to_string();
        let value = value.trim().to_string();

        if value.is_empty() {
            return;
        }

        match step {
            AddProviderPromptStep::ProviderId => {
                if self.provider_catalog.iter().any(|p| p.id == value)
                    || self.custom_providers_list.iter().any(|p| p.id == value)
                {
                    self.messages.push(ChatMessage {
                        role: ChatRole::System,
                        content: format!(
                            "Provider ID '{}' already exists. Choose another ID.",
                            value
                        ),
                        tool_call_id: None,
                        tool_calls: vec![],
                    });
                    self.save_current_chat();
                    self.scroll_messages_to_bottom();
                    cx.notify();
                    return;
                }
                pending.id = value;
            }
            AddProviderPromptStep::ProviderLabel => {
                pending.label = value;
            }
            AddProviderPromptStep::Endpoint => {
                pending.endpoint = value;
            }
            AddProviderPromptStep::ModelId => {
                pending.model_id = value;
            }
            AddProviderPromptStep::ModelLabel => {
                pending.model_label = value;
            }
            AddProviderPromptStep::ModelSupportsTools => {
                let normalized = value.to_ascii_lowercase();
                pending.model_supports_tools = match normalized.as_str() {
                    "y" | "yes" | "true" | "1" => true,
                    "n" | "no" | "false" | "0" => false,
                    _ => {
                        self.messages.push(ChatMessage {
                            role: ChatRole::System,
                            content: "Enter yes/no for tools support.".to_string(),
                            tool_call_id: None,
                            tool_calls: vec![],
                        });
                        self.save_current_chat();
                        self.scroll_messages_to_bottom();
                        cx.notify();
                        return;
                    }
                };
            }
        }

        self.custom_provider_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });

        if let Some(next_step) = Self::next_add_provider_step(step) {
            self.pending_custom_provider_step = Some(next_step);
            cx.notify();
            return;
        }

        let provider = CustomProvider {
            id: pending.id.clone(),
            label: pending.label.clone(),
            endpoint: pending.endpoint.clone(),
            models: vec![crate::custom_providers::CustomModel {
                id: pending.model_id.clone(),
                label: pending.model_label.clone(),
                supports_tools: pending.model_supports_tools,
            }],
        };

        match custom_providers::add_custom_provider(
            &Self::custom_provider_config_dir(),
            provider.clone(),
        ) {
            Ok(()) => {
                let provider_definition = Self::custom_provider_to_definition(&provider);
                self.custom_provider_ids
                    .borrow_mut()
                    .insert(provider.id.clone());
                self.custom_providers_list.push(provider);
                if let Some(saved_provider) = self.custom_providers_list.last() {
                    let models = saved_provider
                        .models
                        .iter()
                        .map(|model| (model.id.clone(), model.label.clone(), model.supports_tools))
                        .collect::<Vec<_>>();
                    self.provider_registry.register(Arc::new(
                        OpenAiCompatibleProvider::from_dynamic_ollama(
                            saved_provider.id.clone(),
                            saved_provider.label.clone(),
                            saved_provider.endpoint.clone(),
                            agent_chat_core::ProviderKind::Local,
                            models,
                        ),
                    ));
                }
                self.provider_catalog.push(provider_definition);
                self.provider_list.update(cx, |list, cx| {
                    list.set_items(self.provider_catalog.clone(), cx);
                });

                self.pending_custom_provider = None;
                self.pending_custom_provider_step = None;

                let new_ix = self.provider_catalog.len().saturating_sub(1);
                self.set_provider(new_ix, cx);

                self.messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: "Custom provider saved to JSON and added to the provider list."
                        .to_string(),
                    tool_call_id: None,
                    tool_calls: vec![],
                });
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                self.scroll_messages_to_bottom();
                cx.notify();
            }
            Err(err) => {
                self.messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: format!("Failed to save custom provider: {err}"),
                    tool_call_id: None,
                    tool_calls: vec![],
                });
                self.save_current_chat();
                self.scroll_messages_to_bottom();
                cx.notify();
            }
        }
    }

    pub(super) fn delete_custom_provider(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        if !self.custom_provider_ids.borrow().contains(provider_id) {
            return;
        }

        match custom_providers::remove_custom_provider(
            &Self::custom_provider_config_dir(),
            provider_id,
        ) {
            Ok(()) => {
                let previous_active_id = self
                    .active_provider()
                    .map(|provider| provider.id.to_string());

                self.custom_provider_ids.borrow_mut().remove(provider_id);
                self.custom_providers_list
                    .retain(|provider| provider.id != provider_id);
                self.provider_catalog
                    .retain(|provider| provider.id != provider_id);

                self.provider_list.update(cx, |list, cx| {
                    list.set_items(self.provider_catalog.clone(), cx);
                });

                if !self.provider_catalog.is_empty() {
                    let fallback_ix = previous_active_id
                        .as_deref()
                        .and_then(|id| {
                            self.provider_catalog
                                .iter()
                                .position(|provider| provider.id == id)
                        })
                        .unwrap_or(0);
                    self.set_provider(fallback_ix, cx);
                }

                self.messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: format!("Custom provider '{}' deleted.", provider_id),
                    tool_call_id: None,
                    tool_calls: vec![],
                });
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                self.scroll_messages_to_bottom();
                cx.notify();
            }
            Err(err) => {
                self.messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: format!("Failed to delete custom provider '{}': {err}", provider_id),
                    tool_call_id: None,
                    tool_calls: vec![],
                });
                self.save_current_chat();
                self.scroll_messages_to_bottom();
                cx.notify();
            }
        }
    }
}
