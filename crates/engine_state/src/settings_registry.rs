use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// The scope of a setting - either global (engine-wide) or project-specific
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SettingScope {
    Global,
    Project,
}

/// The type of a setting field, defining how it should be rendered in the UI
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FieldType {
    Checkbox,
    NumberInput { min: Option<f64>, max: Option<f64>, step: Option<f64> },
    TextInput { placeholder: Option<String>, multiline: bool },
    Dropdown { options: Vec<DropdownOption> },
    Slider { min: f64, max: f64, step: f64 },
    ColorPicker,
    PathSelector { directory: bool },
}

/// An option for a dropdown field
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropdownOption {
    pub label: String,
    pub value: String,
}

/// The value of a setting
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SettingValue {
    Bool(bool),
    Number(f64),
    String(String),
}

impl SettingValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            SettingValue::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            SettingValue::Number(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_string(&self) -> Option<&str> {
        match self {
            SettingValue::String(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        self.as_number().map(|n| n as u32)
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_number().map(|n| n as f32)
    }
}

impl From<bool> for SettingValue {
    fn from(v: bool) -> Self {
        SettingValue::Bool(v)
    }
}

impl From<f64> for SettingValue {
    fn from(v: f64) -> Self {
        SettingValue::Number(v)
    }
}

impl From<f32> for SettingValue {
    fn from(v: f32) -> Self {
        SettingValue::Number(v as f64)
    }
}

impl From<i32> for SettingValue {
    fn from(v: i32) -> Self {
        SettingValue::Number(v as f64)
    }
}

impl From<u32> for SettingValue {
    fn from(v: u32) -> Self {
        SettingValue::Number(v as f64)
    }
}

impl From<String> for SettingValue {
    fn from(v: String) -> Self {
        SettingValue::String(v)
    }
}

impl From<&str> for SettingValue {
    fn from(v: &str) -> Self {
        SettingValue::String(v.to_string())
    }
}

/// Definition of a single setting
#[derive(Debug, Clone)]
pub struct SettingDefinition {
    /// Unique key for this setting (e.g., "editor.font_size")
    pub key: String,
    /// Human-readable label
    pub label: String,
    /// Description of what this setting does
    pub description: String,
    /// The page this setting belongs to
    pub page: String,
    /// The scope of this setting
    pub scope: SettingScope,
    /// The type of field to render
    pub field_type: FieldType,
    /// Default value
    pub default_value: SettingValue,
}

impl SettingDefinition {
    pub fn builder(key: impl Into<String>) -> SettingDefinitionBuilder {
        SettingDefinitionBuilder {
            key: key.into(),
            label: None,
            description: None,
            page: None,
            scope: SettingScope::Global,
            field_type: None,
            default_value: None,
        }
    }
}

/// Builder for SettingDefinition
pub struct SettingDefinitionBuilder {
    key: String,
    label: Option<String>,
    description: Option<String>,
    page: Option<String>,
    scope: SettingScope,
    field_type: Option<FieldType>,
    default_value: Option<SettingValue>,
}

impl SettingDefinitionBuilder {
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn page(mut self, page: impl Into<String>) -> Self {
        self.page = Some(page.into());
        self
    }

    pub fn scope(mut self, scope: SettingScope) -> Self {
        self.scope = scope;
        self
    }

    pub fn field_type(mut self, field_type: FieldType) -> Self {
        self.field_type = Some(field_type);
        self
    }

    pub fn default_value(mut self, value: impl Into<SettingValue>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    pub fn build(self) -> SettingDefinition {
        SettingDefinition {
            key: self.key.clone(),
            label: self.label.unwrap_or_else(|| self.key.clone()),
            description: self.description.unwrap_or_default(),
            page: self.page.unwrap_or_else(|| "General".to_string()),
            scope: self.scope,
            field_type: self.field_type.unwrap_or(FieldType::TextInput { placeholder: None, multiline: false }),
            default_value: self.default_value.unwrap_or(SettingValue::String(String::new())),
        }
    }
}

/// The central registry for all settings
pub struct SettingsRegistry {
    definitions: HashMap<String, SettingDefinition>,
}

impl SettingsRegistry {
    pub fn new() -> Self {
        Self {
            definitions: HashMap::new(),
        }
    }

    /// Register a new setting definition
    pub fn register(&mut self, definition: SettingDefinition) {
        self.definitions.insert(definition.key.clone(), definition);
    }

    /// Get a setting definition by key
    pub fn get(&self, key: &str) -> Option<&SettingDefinition> {
        self.definitions.get(key)
    }

    /// Get all setting definitions for a specific scope
    pub fn get_by_scope(&self, scope: SettingScope) -> Vec<&SettingDefinition> {
        self.definitions
            .values()
            .filter(|def| def.scope == scope)
            .collect()
    }

    /// Get all setting definitions for a specific scope and page
    pub fn get_by_scope_and_page(&self, scope: SettingScope, page: &str) -> Vec<&SettingDefinition> {
        self.definitions
            .values()
            .filter(|def| def.scope == scope && def.page == page)
            .collect()
    }

    /// Get all unique page names for a specific scope
    pub fn get_pages(&self, scope: SettingScope) -> Vec<String> {
        let mut pages: Vec<String> = self.definitions
            .values()
            .filter(|def| def.scope == scope)
            .map(|def| def.page.clone())
            .collect();
        pages.sort();
        pages.dedup();
        pages
    }

    /// Get all setting definitions
    pub fn all(&self) -> Vec<&SettingDefinition> {
        self.definitions.values().collect()
    }
}

impl Default for SettingsRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global settings registry instance
static GLOBAL_REGISTRY: std::sync::LazyLock<Arc<RwLock<SettingsRegistry>>> =
    std::sync::LazyLock::new(|| Arc::new(RwLock::new(SettingsRegistry::new())));

/// Get the global settings registry
pub fn registry() -> Arc<RwLock<SettingsRegistry>> {
    GLOBAL_REGISTRY.clone()
}

/// Register a setting in the global registry
pub fn register_setting(definition: SettingDefinition) {
    GLOBAL_REGISTRY.write().unwrap().register(definition);
}
