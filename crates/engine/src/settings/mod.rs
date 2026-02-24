pub mod engine_settings;

pub use engine_settings::EngineSettings;

// Re-export settings system from engine_state to avoid circular dependencies
pub use engine_state::{
    DropdownOption, FieldType, SettingDefinition, SettingScope, SettingValue,
    SettingsRegistry, register_setting, registry,
    GlobalSettings, ProjectSettings, SettingsStorage,
    register_default_settings,
};
