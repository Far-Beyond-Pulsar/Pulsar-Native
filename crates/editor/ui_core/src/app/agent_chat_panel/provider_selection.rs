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

            // Cancel any stale config flow when switching providers
            self.configuring_provider = None;
            self.config_error = None;

            // If provider is marked Ready, re-validate to catch expired tokens
            if self.provider_states.get(provider_id) == Some(&ProviderState::Ready) {
                if let Some(provider_impl) = self.provider_registry.get(provider_id) {
                    if let Err(e) = provider_impl.validate_config() {
                        self.provider_states.insert(provider_id.to_string(), ProviderState::Unconfigured);
                        self.provider_states_shared.borrow_mut().insert(provider_id.to_string(), ProviderState::Unconfigured);
                        if let Some(entry) = self.provider_entries.get(provider_id) {
                            if !entry.config_fields.is_empty() {
                                self.model_list.update(cx, |list, cx| list.set_items(vec![], cx));
                                self.configuring_provider = Some(provider_id.to_string());
                                self.configuring_field_index = 0;
                                self.config_values.clear();
                                self.config_error = Some(format!("Token expired or invalid: {e}"));
                                cx.notify();
                                return;
                            }
                        }
                    }
                }
            }

            // If unconfigured, start config flow
            if self.provider_states.get(provider_id) == Some(&ProviderState::Unconfigured) {
                if let Some(entry) = self.provider_entries.get(provider_id) {
                    if !entry.config_fields.is_empty() {
                        self.model_list.update(cx, |list, cx| list.set_items(vec![], cx));
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

    /// Refresh just the model list for the current provider without changing
    /// the active provider index.  Safe to call from config form error paths.
    pub(super) fn catalog_for_current_provider(&mut self, cx: &mut Context<Self>) {
        if let Some(provider) = self.active_provider() {
            let models = provider.models.as_ref().clone();
            self.model_list.update(cx, |list, cx| {
                list.set_items(models, cx);
            });
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
                    match result {
                        Ok(models) => {
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
                            if provider_ix < panel.provider_catalog.len() {
                                panel.provider_catalog[provider_ix].models = Arc::new(defs.clone());
                            }
                            if panel.active_provider_ix == provider_ix {
                                panel
                                    .model_list
                                    .update(cx, |list, cx| list.set_items(defs, cx));
                                cx.notify();
                            }
                        }
                        Err(e) => {
                            let label = panel.provider_catalog.get(provider_ix).map(|p| p.label).unwrap_or(provider_id);
                            panel.messages.push(ChatMessage {
                                role: ChatRole::System,
                                content: format!("Failed to fetch models for {label}: {e}"),
                                tool_call_id: None,
                                tool_calls: vec![],
                            });
                            panel.scroll_messages_to_bottom();
                            cx.notify();
                        }
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

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { provider_impl.models() })
                .await;

            cx.update(|cx| {
                this.update(cx, |panel, cx| {
                    match result {
                        Ok(models) => {
                            if models.is_empty() {
                                panel.messages.push(ChatMessage {
                                    role: ChatRole::System,
                                    content: format!("{provider_label} returned no models."),
                                    tool_call_id: None,
                                    tool_calls: vec![],
                                });
                            } else {
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

                                if provider_ix < panel.provider_catalog.len() {
                                    panel.provider_catalog[provider_ix].models = Arc::new(refreshed_models.clone());
                                }
                                panel.active_model_ix = 0;
                                panel.model_list.update(cx, |list, cx| {
                                    list.set_items(refreshed_models, cx);
                                });

                                panel.messages.push(ChatMessage {
                                    role: ChatRole::System,
                                    content: format!("Refreshed models for {provider_label} ({} total).", models.len()),
                                    tool_call_id: None,
                                    tool_calls: vec![],
                                });
                            }
                        }
                        Err(err) => {
                            panel.messages.push(ChatMessage {
                                role: ChatRole::System,
                                content: format!("Failed to refresh models for {provider_label}: {err}"),
                                tool_call_id: None,
                                tool_calls: vec![],
                            });
                        }
                    }
                    panel.scroll_messages_to_bottom();
                    panel.save_current_chat();
                    panel.refresh_chat_history_list(cx);
                    cx.notify();
                });
            });
        }).detach();
    }
}
