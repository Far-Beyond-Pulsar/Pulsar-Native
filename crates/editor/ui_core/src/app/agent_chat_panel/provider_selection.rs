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
            self.active_provider_ix = index;
            self.active_model_ix = 0;

            let provider_id = self.provider_catalog[index].id;

            // If unconfigured, start config flow
            if self.provider_states.get(provider_id) == Some(&ProviderState::Unconfigured) {
                if let Some(entry) = self.provider_entries.get(provider_id) {
                    if !entry.config_fields.is_empty() {
                        self.configuring_provider = Some(provider_id.to_string());
                        self.configuring_field_index = 0;
                        self.config_values.clear();
                        cx.notify();
                        return;
                    }
                }
            }

            let models = self
                .active_provider()
                .map(|provider| provider.models.as_ref().clone())
                .unwrap_or_default();
            self.model_list.update(cx, |list, cx| {
                list.set_items(models.clone(), cx);
            });

            if models.is_empty() {
                self.fetch_models_in_background(index, cx);
            }

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

    /// Fetches models for `provider_ix` on a background thread via `list_models_api`
    /// and updates the catalog + model list when done.  Safe to call at any time;
    /// stale results are discarded if the user has already switched providers.
    pub(super) fn fetch_models_in_background(
        &mut self,
        provider_ix: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(provider_id) = self.provider_catalog.get(provider_ix).map(|p| p.id) else {
            return;
        };
        let Some(provider_impl) = self.provider_registry.get(provider_id).cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { provider_impl.models() })
                .await;

            cx.update(|cx| {
                this.update(cx, |panel, cx| {
                    let Ok(models) = result else { return };
                    if models.is_empty() {
                        return;
                    }
                    let defs = models
                        .iter()
                        .map(|m| ModelDefinition {
                            id: Self::static_str(m.id.to_string()),
                            label: Self::static_str(m.label.to_string()),
                            supports_tools: m.supports_tools,
                            context_tokens: m.context_tokens,
                            compact_model: None,
                        })
                        .collect::<Vec<_>>();
                    panel.provider_catalog[provider_ix].models = Arc::new(defs.clone());
                    if panel.active_provider_ix == provider_ix {
                        panel
                            .model_list
                            .update(cx, |list, cx| list.set_items(defs, cx));
                        cx.notify();
                    }
                });
            });
        })
        .detach();
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

        match provider_impl.models() {
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
                        compact_model: model.compact_model.as_ref().map(|s| Self::static_str(s.clone())),
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
