use super::*;

impl AgentChatPanel {
    pub(super) fn active_provider(&self) -> Option<&ProviderDefinition> {
        self.provider_catalog.get(self.active_provider_ix)
    }

    pub(super) fn active_model(&self) -> Option<&ModelDefinition> {
        self.active_provider()
            .and_then(|provider| provider.models.get(self.active_model_ix))
    }

    pub(super) fn set_provider(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.provider_catalog.len() {
            if self
                .provider_catalog
                .get(index)
                .is_some_and(|provider| self.wip_providers.contains_key(provider.id))
            {
                return;
            }

            self.active_provider_ix = index;
            self.active_model_ix = 0;

            let models = self
                .active_provider()
                .map(|provider| provider.models.as_ref().clone())
                .unwrap_or_default();
            self.model_list.update(cx, |list, cx| {
                list.set_items(models, cx);
            });

            self.maybe_require_auth_for_active_provider(cx);

            cx.notify();
        }
    }

    pub(super) fn set_model(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(provider) = self.active_provider() {
            if index < provider.models.len() {
                self.active_model_ix = index;
                cx.notify();
            }
        }
    }

    pub(super) fn refresh_models_for_active_provider(&mut self, cx: &mut Context<Self>) {
        let Some(provider_ix) = self
            .provider_catalog
            .get(self.active_provider_ix)
            .map(|_| self.active_provider_ix)
        else {
            return;
        };

        let provider_id = self.provider_catalog[provider_ix].id;
        let provider_label = self.provider_catalog[provider_ix].label;

        let Some(provider_impl) = self.provider_registry.get(provider_id).cloned() else {
            self.messages.push(ChatMessage {
                role: ChatRole::System,
                content: format!(
                    "Cannot refresh models for {} because no runtime provider is registered.",
                    provider_label
                ),
                tool_call_id: None,
                tool_calls: vec![],
            });
            self.scroll_messages_to_bottom();
            self.save_current_chat();
            self.refresh_chat_history_list(cx);
            cx.notify();
            return;
        };

        let token = self
            .auth_token_for_provider(provider_id)
            .unwrap_or_default();
        match provider_impl.list_models_api(&token) {
            Ok(models) => {
                if models.is_empty() {
                    self.messages.push(ChatMessage {
                        role: ChatRole::System,
                        content: format!("{} returned no models.", provider_label),
                        tool_call_id: None,
                        tool_calls: vec![],
                    });
                    self.scroll_messages_to_bottom();
                    self.save_current_chat();
                    self.refresh_chat_history_list(cx);
                    cx.notify();
                    return;
                }

                let refreshed_models = models
                    .iter()
                    .map(|model| ModelDefinition {
                        id: Self::static_str(model.id.to_string()),
                        label: Self::static_str(model.label.to_string()),
                        supports_tools: model.supports_tools,
                        context_tokens: model.context_tokens,
                    })
                    .collect::<Vec<_>>();

                self.provider_catalog[provider_ix].models = Arc::new(refreshed_models.clone());
                self.active_model_ix = 0;
                self.model_list.update(cx, |list, cx| {
                    list.set_items(refreshed_models, cx);
                });

                self.messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: format!(
                        "Refreshed models for {} ({} total).",
                        provider_label,
                        models.len()
                    ),
                    tool_call_id: None,
                    tool_calls: vec![],
                });
                self.scroll_messages_to_bottom();
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                cx.notify();
            }
            Err(err) => {
                self.messages.push(ChatMessage {
                    role: ChatRole::System,
                    content: format!("Failed to refresh models for {}: {err}", provider_label),
                    tool_call_id: None,
                    tool_calls: vec![],
                });
                self.scroll_messages_to_bottom();
                self.save_current_chat();
                self.refresh_chat_history_list(cx);
                cx.notify();
            }
        }
    }
}
