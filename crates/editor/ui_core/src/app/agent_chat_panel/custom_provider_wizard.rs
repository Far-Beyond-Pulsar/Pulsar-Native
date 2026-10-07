//! Adding, saving and deleting provider connections.
//!
//! Providers come in two flavours. A normal provider is configured in place.
//! A template (`ProviderEntry::template`, e.g. LM Studio or Ollama) is never
//! used directly: configuring it asks for a connection name first, and the
//! result is saved as a new named provider in the engine's data dir
//! (`custom_providers`), which shows in the provider list with Delete.

use super::*;
use crate::custom_providers;
use agent_chat_core::{ConfigField, ProviderInstance, ProviderInstanceConfig};
use std::sync::Arc;

/// Config key of the connection-name step shown before a template's fields.
pub(super) const CONNECTION_NAME_KEY: &str = "__connection_name";

const CONNECTION_NAME_FIELD: ConfigField = ConfigField {
    key: CONNECTION_NAME_KEY,
    label: "Connection name",
    description: "Name this connection, e.g. \"Desk GPU\". It appears in the provider list.",
    sensitive: false,
    required: true,
    placeholder: None,
};

/// Template the header's "Add Provider" configures: any OpenAI-compatible server.
const ADD_PROVIDER_TEMPLATE: &str = "custom_openai";

impl AgentChatPanel {
    /// The steps of the config flow for `id`: a template asks for a name first.
    pub(super) fn config_fields_for(&self, id: &str) -> Vec<ConfigField> {
        let Some(entry) = self.provider_entries.get(id) else {
            return Vec::new();
        };
        let mut fields = entry.config_fields.clone();
        if entry.template {
            fields.insert(0, CONNECTION_NAME_FIELD);
        }
        fields
    }

    /// "Add Provider": configure a new OpenAI-compatible connection. Other
    /// templates (LM Studio, Ollama, ...) are picked from the provider list.
    pub(super) fn start_add_provider_prompt(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_provider_config(ADD_PROVIDER_TEMPLATE, cx);
    }

    pub(super) fn start_provider_config(&mut self, id: &str, cx: &mut Context<Self>) {
        self.model_list
            .update(cx, |list, cx| list.set_items(vec![], cx));
        self.configuring_provider = Some(id.to_string());
        self.configuring_field_index = 0;
        self.config_values.clear();
        self.config_error = None;
        cx.notify();
    }

    /// Create a template's provider under the instance's id and name.
    pub(super) fn instantiate_provider(
        crates: &[Box<dyn ProviderCrate>],
        instance: &ProviderInstanceConfig,
    ) -> anyhow::Result<Box<dyn ChatProvider>> {
        let provider_crate = crates
            .iter()
            .find(|c| c.entries().iter().any(|e| e.id == instance.template_id))
            .ok_or_else(|| {
                anyhow::anyhow!("unknown provider template '{}'", instance.template_id)
            })?;
        let inner = provider_crate.create(&instance.template_id, instance.provider_config())?;
        Ok(Box::new(ProviderInstance::new(instance, inner)))
    }

    /// The entry a saved connection is configured with: its template's
    /// fields, under the connection's id, no longer a template.
    pub(super) fn instance_entry(
        template: &ProviderEntry,
        instance: &ProviderInstanceConfig,
    ) -> ProviderEntry {
        ProviderEntry {
            id: Self::static_str(instance.id.clone()),
            display_name: Self::static_str(instance.name.clone()),
            template: false,
            ..template.clone()
        }
    }

    /// Finish the config flow for `id` with the collected `values`: a
    /// template becomes a new saved connection, a saved connection is
    /// updated, anything else is (re)configured in place. The provider must
    /// validate (e.g. the server answers) before anything is saved.
    pub(super) fn apply_provider_config(
        &mut self,
        id: &str,
        mut values: HashMap<String, String>,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        // Optional fields left empty mean "use the default".
        values.retain(|_, v| !v.trim().is_empty());
        let is_template = self.provider_entries.get(id).is_some_and(|e| e.template);

        if is_template {
            let template = &self.provider_entries[id];
            let name = values
                .remove(CONNECTION_NAME_KEY)
                .unwrap_or_else(|| template.display_name.to_string());
            let taken: Vec<&str> = self
                .provider_instances
                .iter()
                .map(|i| i.id.as_str())
                .chain(self.provider_entries.keys().map(String::as_str))
                .collect();
            let instance = ProviderInstanceConfig::new(id, &name, values, &taken);
            self.save_provider_instance(instance, cx)
        } else if let Some(existing) = self.provider_instances.iter().find(|i| i.id == id) {
            let instance = ProviderInstanceConfig {
                values,
                ..existing.clone()
            };
            self.save_provider_instance(instance, cx)
        } else {
            let config = agent_chat_core::ProviderConfig { values };
            let provider_crate = self
                .crate_instances
                .iter()
                .find(|c| c.entries().iter().any(|e| e.id == id))
                .ok_or_else(|| anyhow::anyhow!("unknown provider '{id}'"))?;
            let provider = provider_crate.create(id, config)?;
            provider.validate_config()?;
            self.provider_registry.register(Arc::from(provider));
            self.set_provider_state(id, ProviderState::Ready);
            self.provider_entries.remove(id);
            self.refresh_provider_catalog(cx);
            Ok(())
        }
    }

    /// Validate, register and persist a (new or edited) saved connection,
    /// then select it.
    fn save_provider_instance(
        &mut self,
        instance: ProviderInstanceConfig,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let provider = Self::instantiate_provider(&self.crate_instances, &instance)?;
        provider.validate_config()?;

        let mut instances = self.provider_instances.clone();
        match instances.iter_mut().find(|i| i.id == instance.id) {
            Some(existing) => *existing = instance.clone(),
            None => instances.push(instance.clone()),
        }
        custom_providers::save_provider_instances(&instances)?;
        self.provider_instances = instances;

        if let Some(template) = self.provider_entries.get(&instance.template_id).cloned() {
            self.provider_entries.insert(
                instance.id.clone(),
                Self::instance_entry(&template, &instance),
            );
        }
        self.provider_registry.register(Arc::from(provider));
        self.set_provider_state(&instance.id, ProviderState::Ready);
        self.refresh_provider_catalog(cx);
        if let Some(ix) = self
            .provider_catalog
            .iter()
            .position(|p| p.id == instance.id)
        {
            self.active_provider_ix = ix;
        }
        Ok(())
    }

    pub(super) fn set_provider_state(&mut self, id: &str, state: ProviderState) {
        self.provider_states.insert(id.to_string(), state.clone());
        self.provider_states_shared
            .borrow_mut()
            .insert(id.to_string(), state);
    }

    /// Delete a saved connection (the provider list's Delete action).
    pub(super) fn delete_custom_provider(&mut self, provider_id: &str, cx: &mut Context<Self>) {
        tracing::debug!(provider = %provider_id, "delete_custom_provider: deleting");
        let mut instances = self.provider_instances.clone();
        instances.retain(|p| p.id != provider_id);
        if let Err(e) = custom_providers::save_provider_instances(&instances) {
            tracing::error!("failed to delete provider connection: {e}");
            return;
        }
        self.provider_instances = instances;
        self.provider_registry.remove(provider_id);
        self.provider_entries.remove(provider_id);
        self.provider_states.remove(provider_id);
        self.provider_states_shared.borrow_mut().remove(provider_id);
        if self.configuring_provider.as_deref() == Some(provider_id) {
            self.configuring_provider = None;
            self.config_error = None;
        }
        self.refresh_provider_catalog(cx);
    }

    /// The provider list: every registered provider plus one "New …
    /// connection" item per template, ordered Ready, Unconfigured, templates,
    /// Disabled, alphabetical within each.
    pub(super) fn build_provider_catalog(
        registry: &ProviderRegistry,
        entries: &HashMap<String, ProviderEntry>,
        states: &HashMap<String, ProviderState>,
        instances: &[ProviderInstanceConfig],
        old_models: &HashMap<&str, Arc<Vec<ModelDefinition>>>,
    ) -> Vec<ProviderDefinition> {
        let kind_of = |id: &str| match entries.get(id).map(|e| e.kind) {
            Some(agent_chat_core::ProviderKind::Local) => ProviderKind::Local,
            _ => ProviderKind::Cloud,
        };
        let mut catalog: Vec<ProviderDefinition> = registry
            .all()
            .map(|(id, provider)| ProviderDefinition {
                id: Self::static_str(id.clone()),
                label: Self::static_str(provider.display_name().to_string()),
                kind: kind_of(id),
                endpoint: "",
                models: old_models
                    .get(id.as_str())
                    .cloned()
                    .unwrap_or_else(|| Arc::new(vec![])),
                deletable: instances.iter().any(|i| &i.id == id),
            })
            .collect();
        catalog.extend(
            entries
                .values()
                .filter(|e| e.template)
                .map(|e| ProviderDefinition {
                    id: e.id,
                    label: Self::static_str(format!("New {} connection…", e.display_name)),
                    kind: kind_of(e.id),
                    endpoint: e.default_endpoint.unwrap_or(""),
                    models: Arc::new(vec![]),
                    deletable: false,
                }),
        );

        let state_order = |id: &str| -> u8 {
            match states.get(id) {
                Some(ProviderState::Ready) => 0,
                Some(ProviderState::Unconfigured) => 1,
                Some(ProviderState::Template) => 2,
                Some(ProviderState::Disabled) | None => 3,
            }
        };
        catalog.sort_by(|a, b| {
            state_order(a.id)
                .cmp(&state_order(b.id))
                .then_with(|| a.label.cmp(b.label))
        });
        catalog
    }

    pub(super) fn refresh_provider_catalog(&mut self, cx: &mut Context<Self>) {
        let current_id = self.active_provider().map(|p| p.id.to_string());
        tracing::debug!(
            "refresh_provider_catalog: rebuilding, previous size={}",
            self.provider_catalog.len()
        );

        // Preserve models that were already fetched across catalog refreshes
        let old_models: HashMap<&str, Arc<Vec<ModelDefinition>>> = self
            .provider_catalog
            .iter()
            .map(|p| (p.id, p.models.clone()))
            .collect();
        let catalog = Self::build_provider_catalog(
            &self.provider_registry,
            &self.provider_entries,
            &self.provider_states,
            &self.provider_instances,
            &old_models,
        );

        self.provider_catalog = catalog;
        // Push the re-sorted catalog into the list entity so the UI reflects it
        self.provider_list.update(cx, |list, cx| {
            list.set_items(self.provider_catalog.clone(), cx);
        });
        // Restore selection by provider ID — never change provider automatically
        self.active_provider_ix = current_id
            .and_then(|id| self.provider_catalog.iter().position(|p| p.id == id))
            .unwrap_or(0);
        cx.notify();
        tracing::debug!(
            count = self.provider_catalog.len(),
            "refresh_provider_catalog: built"
        );
    }
}
