//! Saved AI provider connections: configured provider templates
//! (`ProviderEntry::template`), each a named, deletable provider.
//!
//! Stored in the engine's app data directory at
//! `<data dir>/configs/ai_providers.json`, next to `engine.toml`.

use agent_chat_core::ProviderInstanceConfig;
use directories::ProjectDirs;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const PROVIDERS_FILE: &str = "ai_providers.json";

/// Template that pre-template "custom providers" (an OpenAI-compatible
/// endpoint) become.
const LEGACY_TEMPLATE: &str = "custom_openai";

pub fn providers_file() -> Option<PathBuf> {
    ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|dirs| dirs.data_dir().join("configs").join(PROVIDERS_FILE))
}

/// Custom providers saved before templates existed, in the OS config dir.
#[derive(Deserialize)]
struct LegacyCustomProvider {
    label: String,
    endpoint: String,
}

#[derive(Deserialize)]
struct LegacyCustomProvidersConfig {
    providers: Vec<LegacyCustomProvider>,
}

fn legacy_file() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("pulsar").join("custom_providers.json"))
}

/// Load saved connections, moving any legacy custom providers into the store
/// (the legacy file is removed once they are saved).
pub fn load_provider_instances() -> Vec<ProviderInstanceConfig> {
    let Some(path) = providers_file() else {
        return Vec::new();
    };
    let mut instances = match agent_chat_core::load_provider_instances(&path) {
        Ok(instances) => instances,
        Err(e) => {
            tracing::warn!("Failed to read {}: {e}", path.display());
            return Vec::new();
        }
    };

    let Some(legacy_path) = legacy_file().filter(|p| p.exists()) else {
        return instances;
    };
    let legacy = fs::read_to_string(&legacy_path)
        .ok()
        .and_then(|text| serde_json::from_str::<LegacyCustomProvidersConfig>(&text).ok());
    if let Some(legacy) = legacy {
        for provider in legacy.providers {
            let taken: Vec<&str> = instances.iter().map(|i| i.id.as_str()).collect();
            let values = HashMap::from([("endpoint_url".to_string(), provider.endpoint)]);
            let instance =
                ProviderInstanceConfig::new(LEGACY_TEMPLATE, &provider.label, values, &taken);
            instances.push(instance);
        }
        if save_provider_instances(&instances).is_ok() {
            let _ = fs::remove_file(&legacy_path);
        }
    }
    instances
}

pub fn save_provider_instances(instances: &[ProviderInstanceConfig]) -> anyhow::Result<()> {
    let path = providers_file().ok_or_else(|| anyhow::anyhow!("No app data directory"))?;
    agent_chat_core::save_provider_instances(&path, instances)
}
