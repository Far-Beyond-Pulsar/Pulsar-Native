pub mod engine_settings;

pub use engine_settings::EngineSettings;

// Re-export settings system from engine_state to avoid circular dependencies
pub use engine_state::{
    ChangeEvent, Color, ConfigError, ConfigManager, ConfigStore, ConfigValue, DropdownOption,
    FieldType, GlobalSettings, ListenerId, NS_EDITOR, NS_PROJECT, NamespaceSchema, OwnerHandle,
    PersistError, ProjectSettings, SchemaEntry, SearchResult, SettingInfo, Validator,
    global_config, register_default_settings,
};
