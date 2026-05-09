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
}
