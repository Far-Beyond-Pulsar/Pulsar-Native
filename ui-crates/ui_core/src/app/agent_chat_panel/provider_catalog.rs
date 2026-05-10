use super::*;
use crate::custom_providers::CustomProvider;
use std::sync::Arc;

impl AgentChatPanel {
    /// Leak a heap-allocated string into a `&'static str`.
    /// Used when dynamic provider metadata needs to live as long as the program.
    pub(super) fn static_str(value: String) -> &'static str {
        Box::leak(value.into_boxed_str())
    }

    pub(super) fn custom_provider_to_definition(provider: &CustomProvider) -> ProviderDefinition {
        let models = provider
            .models
            .iter()
            .map(|model| ModelDefinition {
                id: Self::static_str(model.id.clone()),
                label: Self::static_str(model.label.clone()),
                supports_tools: model.supports_tools,
                context_tokens: 0,
            })
            .collect::<Vec<_>>();

        ProviderDefinition {
            id: Self::static_str(provider.id.clone()),
            label: Self::static_str(provider.label.clone()),
            kind: ProviderKind::Local,
            endpoint: Self::static_str(provider.endpoint.clone()),
            models: Arc::new(models),
        }
    }
}
