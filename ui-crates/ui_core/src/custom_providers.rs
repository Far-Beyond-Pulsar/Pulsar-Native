use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub models: Vec<CustomModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomModel {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub supports_tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvidersConfig {
    pub providers: Vec<CustomProvider>,
}

const CUSTOM_PROVIDERS_FILE: &str = "custom_providers.json";

/// Load custom providers from the app data folder
pub fn load_custom_providers(app_data_dir: &Path) -> Vec<CustomProvider> {
    let config_path = app_data_dir.join(CUSTOM_PROVIDERS_FILE);
    
    match fs::read_to_string(&config_path) {
        Ok(content) => {
            match serde_json::from_str::<CustomProvidersConfig>(&content) {
                Ok(config) => config.providers,
                Err(e) => {
                    tracing::warn!("Failed to parse custom providers config: {}", e);
                    Vec::new()
                }
            }
        }
        Err(_) => {
            // File doesn't exist yet, return empty list
            Vec::new()
        }
    }
}

/// Save custom providers to the app data folder
pub fn save_custom_providers(app_data_dir: &Path, providers: &[CustomProvider]) -> anyhow::Result<()> {
    let config_path = app_data_dir.join(CUSTOM_PROVIDERS_FILE);
    
    // Create app data dir if it doesn't exist
    fs::create_dir_all(app_data_dir)?;
    
    let config = CustomProvidersConfig {
        providers: providers.to_vec(),
    };
    
    let json = serde_json::to_string_pretty(&config)?;
    fs::write(&config_path, json)?;
    
    Ok(())
}

/// Add a new custom provider and save to disk
pub fn add_custom_provider(app_data_dir: &Path, provider: CustomProvider) -> anyhow::Result<()> {
    let mut providers = load_custom_providers(app_data_dir);
    
    // Check if provider with same ID already exists
    if providers.iter().any(|p| p.id == provider.id) {
        return Err(anyhow::anyhow!("Provider with ID '{}' already exists", provider.id));
    }
    
    providers.push(provider);
    save_custom_providers(app_data_dir, &providers)?;
    
    Ok(())
}

/// Remove a custom provider by ID
pub fn remove_custom_provider(app_data_dir: &Path, provider_id: &str) -> anyhow::Result<()> {
    let mut providers = load_custom_providers(app_data_dir);
    providers.retain(|p| p.id != provider_id);
    save_custom_providers(app_data_dir, &providers)?;
    Ok(())
}
