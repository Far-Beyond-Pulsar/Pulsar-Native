use super::*;
use crate::custom_providers::{self, CustomProvider, CustomModel};
use agent_provider_openai::OpenAiProviderCrate;
use std::path::PathBuf;
use std::sync::Arc;

impl AgentChatPanel {
    pub(super) fn custom_provider_config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("pulsar")
    }

    pub(super) fn start_add_provider_prompt(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_custom_provider = Some(PendingCustomProvider::default());
        self.pending_custom_provider_step = Some(AddProviderPromptStep::ProviderLabel);
        cx.notify();
    }

    pub(super) fn cancel_add_provider_prompt(&mut self, _cx: &mut Context<Self>) {
        self.pending_custom_provider = None;
        self.pending_custom_provider_step = None;
    }

    pub(super) fn submit_custom_provider(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pending) = self.pending_custom_provider.take() else { return };
        if pending.label.is_empty() || pending.endpoint.is_empty() {
            self.pending_custom_provider = Some(pending);
            return;
        }
        self.pending_custom_provider_step = None;

        let id = if pending.id.is_empty() {
            pending.label.to_lowercase().replace(' ', "_")
        } else {
            pending.id.clone()
        };

        let custom_provider = CustomProvider {
            id: id.clone(),
            label: pending.label.clone(),
            endpoint: pending.endpoint.clone(),
            models: vec![],
        };

        // Save to disk
        let config_dir = Self::custom_provider_config_dir();
        if let Err(e) = custom_providers::add_custom_provider(&config_dir, custom_provider) {
            tracing::error!("failed to save custom provider: {e}");
            return;
        }

        // Register as a runtime provider
        let config = agent_chat_core::ProviderConfig {
            values: std::collections::HashMap::from([
                ("endpoint_url".to_string(), pending.endpoint),
            ]),
        };
        let openai = OpenAiProviderCrate;
        match openai.create("custom_openai", config) {
            Ok(provider) => {
                self.provider_registry.register(Arc::from(provider));
            }
            Err(e) => {
                tracing::error!("failed to create custom provider: {e}");
            }
        }

        // Refresh the catalog
        self.refresh_provider_catalog(cx);
    }

    pub(super) fn delete_custom_provider(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        let config_dir = Self::custom_provider_config_dir();
        let _ = custom_providers::remove_custom_provider(&config_dir, provider_id);
        self.custom_providers_list
            .retain(|p| p.id != provider_id);
        self.provider_registry.remove(provider_id);
        self.refresh_provider_catalog(cx);
    }

    pub(super) fn refresh_provider_catalog(&mut self, cx: &mut Context<Self>) {
        let old_selection = self.active_provider_ix;
        let mut catalog: Vec<ProviderDefinition> = Vec::new();

        for (id, provider) in self.provider_registry.all() {
            catalog.push(ProviderDefinition {
                id: Box::leak(id.clone().into_boxed_str()),
                label: Box::leak(provider.display_name().to_string().into_boxed_str()),
                kind: ProviderKind::Cloud,
                endpoint: Box::leak(String::new().into_boxed_str()),
                models: Arc::new(vec![]),
            });
        }

        for custom in &self.custom_providers_list {
            let models = custom
                .models
                .iter()
                .map(|m| ModelDefinition {
                    id: Box::leak(m.id.clone().into_boxed_str()),
                    label: Box::leak(m.label.clone().into_boxed_str()),
                    supports_tools: m.supports_tools,
                    context_tokens: 0,
                    compact_model: None,
                })
                .collect::<Vec<_>>();
            catalog.push(ProviderDefinition {
                id: Box::leak(custom.id.clone().into_boxed_str()),
                label: Box::leak(custom.label.clone().into_boxed_str()),
                kind: ProviderKind::Local,
                endpoint: Box::leak(custom.endpoint.clone().into_boxed_str()),
                models: Arc::new(models),
            });
        }

        self.provider_catalog = catalog;
        self.active_provider_ix = old_selection.min(self.provider_catalog.len().saturating_sub(1));
        cx.notify();
    }

    pub(super) fn add_provider_prompt_title(step: AddProviderPromptStep) -> &'static str {
        match step {
            AddProviderPromptStep::ProviderId => "Provider ID (e.g. my_local)",
            AddProviderPromptStep::ProviderLabel => "Provider name (e.g. My Local LLM)",
            AddProviderPromptStep::Endpoint => "Endpoint URL (e.g. http://localhost:8080/v1)",
            AddProviderPromptStep::ModelId => "Default model ID",
            AddProviderPromptStep::ModelLabel => "Default model label",
            AddProviderPromptStep::ModelSupportsTools => "Supports tools? (yes/no)",
        }
    }

    pub(super) fn next_add_provider_step(
        step: AddProviderPromptStep,
    ) -> Option<AddProviderPromptStep> {
        match step {
            AddProviderPromptStep::ProviderLabel => Some(AddProviderPromptStep::Endpoint),
            AddProviderPromptStep::Endpoint => None,
            _ => None,
        }
    }
}
