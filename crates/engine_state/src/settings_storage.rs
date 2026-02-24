use super::settings_registry::{SettingValue, SettingScope, registry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use toml::Value as TomlValue;

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
            Ok(content) => {
                // Try to parse as nested TOML first
                if let Ok(nested) = toml::from_str::<TomlValue>(&content) {
                    Self::from_nested_toml(&nested)
                } else {
                    // Fall back to flat structure
                    toml::from_str(&content).unwrap_or_default()
                }
            }
            Err(_) => Self::default(),
        }
    }

    /// Save settings to a file
    pub fn save(&self, path: &Path) -> Result<(), String> {
        tracing::info!("Saving settings to: {}", path.display());

        if let Some(parent) = path.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                let err_msg = format!("Failed to create config directory: {}", e);
                tracing::error!("{}", err_msg);
                return Err(err_msg);
            }
        }

        // Convert to nested structure for Pulsar.toml
        let nested = self.to_nested_toml();
        let toml = toml::to_string_pretty(&nested)
            .map_err(|e| {
                let err_msg = format!("Failed to serialize settings: {}", e);
                tracing::error!("{}", err_msg);
                err_msg
            })?;

        tracing::debug!("Writing TOML content:\n{}", toml);

        fs::write(path, toml)
            .map_err(|e| {
                let err_msg = format!("Failed to write settings to {}: {}", path.display(), e);
                tracing::error!("{}", err_msg);
                err_msg
            })?;

        tracing::info!("Successfully saved settings to: {}", path.display());
        Ok(())
    }

    /// Convert flat keys to nested TOML structure
    fn to_nested_toml(&self) -> TomlValue {
        let mut root = toml::map::Map::new();

        for (key, value) in &self.values {
            let parts: Vec<&str> = key.split('.').collect();
            Self::insert_nested(&mut root, &parts, value);
        }

        TomlValue::Table(root)
    }

    /// Insert a value into a nested structure
    fn insert_nested(table: &mut toml::map::Map<String, TomlValue>, parts: &[&str], value: &SettingValue) {
        if parts.is_empty() {
            return;
        }

        if parts.len() == 1 {
            // Leaf node - insert the value
            let toml_value = match value {
                SettingValue::Bool(b) => TomlValue::Boolean(*b),
                SettingValue::Number(n) => {
                    // Check if it's an integer
                    if n.fract() == 0.0 && *n >= i64::MIN as f64 && *n <= i64::MAX as f64 {
                        TomlValue::Integer(*n as i64)
                    } else {
                        TomlValue::Float(*n)
                    }
                }
                SettingValue::String(s) => TomlValue::String(s.clone()),
            };
            table.insert(parts[0].to_string(), toml_value);
        } else {
            // Intermediate node - create or get the subtable
            let subtable = table
                .entry(parts[0].to_string())
                .or_insert_with(|| TomlValue::Table(toml::map::Map::new()));

            if let TomlValue::Table(t) = subtable {
                Self::insert_nested(t, &parts[1..], value);
            }
        }
    }

    /// Convert nested TOML structure to flat keys
    fn from_nested_toml(value: &TomlValue) -> Self {
        let mut values = HashMap::new();
        Self::flatten_toml(&mut values, String::new(), value);
        Self { values }
    }

    /// Flatten nested TOML into flat keys
    fn flatten_toml(values: &mut HashMap<String, SettingValue>, prefix: String, value: &TomlValue) {
        match value {
            TomlValue::Table(table) => {
                for (key, val) in table {
                    let new_prefix = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{}.{}", prefix, key)
                    };
                    Self::flatten_toml(values, new_prefix, val);
                }
            }
            TomlValue::Boolean(b) => {
                values.insert(prefix, SettingValue::Bool(*b));
            }
            TomlValue::Integer(i) => {
                values.insert(prefix, SettingValue::Number(*i as f64));
            }
            TomlValue::Float(f) => {
                values.insert(prefix, SettingValue::Number(*f));
            }
            TomlValue::String(s) => {
                values.insert(prefix, SettingValue::String(s.clone()));
            }
            // Ignore arrays and datetime for now
            _ => {}
        }
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
        let path = project_path.join("Pulsar.toml");
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
