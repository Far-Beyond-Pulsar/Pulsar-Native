use super::settings_registry::{SettingValue, SettingScope, registry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Settings storage that handles reading/writing settings to disk
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SettingsStorage {
    /// Map of setting keys to their values
    values: HashMap<String, SettingValue>,
}

impl SettingsStorage {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    /// Load settings from a file
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Save settings to a file
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return Err(format!("Failed to create config directory: {}", e));
            }
        }

        let toml = toml::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;

        fs::write(path, toml)
            .map_err(|e| format!("Failed to write settings: {}", e))?;

        Ok(())
    }

    /// Get a setting value by key
    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.values.get(key)
    }

    /// Get a setting value or return the default from the registry
    pub fn get_or_default(&self, key: &str) -> SettingValue {
        if let Some(value) = self.values.get(key) {
            return value.clone();
        }

        // Fall back to registry default
        registry()
            .read()
            .unwrap()
            .get(key)
            .map(|def| def.default_value.clone())
            .unwrap_or(SettingValue::String(String::new()))
    }

    /// Set a setting value
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<SettingValue>) {
        self.values.insert(key.into(), value.into());
    }

    /// Remove a setting value (will fall back to default)
    pub fn remove(&mut self, key: &str) -> Option<SettingValue> {
        self.values.remove(key)
    }

    /// Get all setting values
    pub fn all(&self) -> &HashMap<String, SettingValue> {
        &self.values
    }

    /// Clear all settings
    pub fn clear(&mut self) {
        self.values.clear();
    }

    /// Initialize with defaults from the registry for a given scope
    pub fn init_with_defaults(&mut self, scope: SettingScope) {
        let reg = registry();
        let reg = reg.read().unwrap();

        for def in reg.get_by_scope(scope) {
            if !self.values.contains_key(&def.key) {
                self.values.insert(def.key.clone(), def.default_value.clone());
            }
        }
    }
}

/// Global settings manager
pub struct GlobalSettings {
    storage: SettingsStorage,
    path: PathBuf,
}

impl GlobalSettings {
    pub fn new() -> Self {
        let path = Self::default_path();
        let mut storage = SettingsStorage::load(&path);
        storage.init_with_defaults(SettingScope::Global);

        Self { storage, path }
    }

    pub fn with_path(path: PathBuf) -> Self {
        let mut storage = SettingsStorage::load(&path);
        storage.init_with_defaults(SettingScope::Global);

        Self { storage, path }
    }

    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .map(|proj| proj.data_dir().join("configs/engine.toml"))
            .unwrap_or_else(|| PathBuf::from("engine.toml"))
    }

    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.storage.get(key)
    }

    pub fn get_or_default(&self, key: &str) -> SettingValue {
        self.storage.get_or_default(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<SettingValue>) {
        self.storage.set(key, value);
    }

    pub fn save(&self) -> Result<(), String> {
        self.storage.save(&self.path)
    }

    pub fn storage(&self) -> &SettingsStorage {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut SettingsStorage {
        &mut self.storage
    }
}

/// Project settings manager
pub struct ProjectSettings {
    storage: SettingsStorage,
    path: PathBuf,
}

impl ProjectSettings {
    pub fn new(project_path: &Path) -> Self {
        let path = project_path.join("project_settings.toml");
        let mut storage = SettingsStorage::load(&path);
        storage.init_with_defaults(SettingScope::Project);

        Self { storage, path }
    }

    pub fn with_path(path: PathBuf) -> Self {
        let mut storage = SettingsStorage::load(&path);
        storage.init_with_defaults(SettingScope::Project);

        Self { storage, path }
    }

    pub fn get(&self, key: &str) -> Option<&SettingValue> {
        self.storage.get(key)
    }

    pub fn get_or_default(&self, key: &str) -> SettingValue {
        self.storage.get_or_default(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<SettingValue>) {
        self.storage.set(key, value);
    }

    pub fn save(&self) -> Result<(), String> {
        self.storage.save(&self.path)
    }

    pub fn storage(&self) -> &SettingsStorage {
        &self.storage
    }

    pub fn storage_mut(&mut self) -> &mut SettingsStorage {
        &mut self.storage
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
