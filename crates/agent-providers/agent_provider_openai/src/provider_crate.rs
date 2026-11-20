//! `OpenAiProviderCrate`: lists the catalog and creates a chat provider for one entry.

use agent_chat_core::{ChatProvider, ProviderConfig, ProviderCrate, ProviderEntry, ProviderKind};
use anyhow::anyhow;

use crate::chat_provider::OpenAiChatProvider;
use crate::entries::{entry_config_fields, resolve_endpoint, ENTRIES};
use crate::transport::http_client;
use crate::urls::{build_chat_url, build_models_url};

pub struct OpenAiProviderCrate;

impl ProviderCrate for OpenAiProviderCrate {
    fn entries(&self) -> Vec<ProviderEntry> {
        ENTRIES
            .iter()
            .map(|e| {
                let is_ollama = e.id == "ollama" || e.id == "custom_openai";
                ProviderEntry {
                    id: e.id,
                    display_name: e.display_name,
                    kind: e.kind,
                    default_endpoint: e.endpoint,
                    config_fields: entry_config_fields(e, is_ollama),
                    template: e.template,
                }
            })
            .collect()
    }

    fn create(&self, id: &str, config: ProviderConfig) -> anyhow::Result<Box<dyn ChatProvider>> {
        let entry = ENTRIES
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| anyhow!("unknown provider id: {id}"))?;

        let endpoint = resolve_endpoint(entry, &config)?;

        let api_key = config.get("api_key").unwrap_or_default().to_string();
        let chat_url = build_chat_url(&endpoint, entry.use_ollama_protocol, entry.chat_path);
        let models_url = build_models_url(&chat_url, entry.use_ollama_protocol);

        let _config_fields = entry_config_fields(entry, entry.use_ollama_protocol);

        Ok(Box::new(OpenAiChatProvider {
            client: http_client()?,
            id: id.to_string(),
            display_name: entry.display_name.to_string(),
            chat_url,
            models_url,
            api_key,
            is_ollama: entry.use_ollama_protocol,
            cloud: entry.kind == ProviderKind::Cloud,
        }))
    }
}
