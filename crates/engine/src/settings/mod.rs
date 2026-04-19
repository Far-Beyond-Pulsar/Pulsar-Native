pub mod engine_settings;

pub use engine_settings::EngineSettings;

// Re-export settings system from engine_state to avoid circular dependencies
pub use engine_state::{
    global_config,
    ChangeEvent, Color, ConfigError, ConfigManager, ConfigStore, ConfigValue,
    DropdownOption, FieldType, ListenerId, NamespaceSchema, OwnerHandle,
    PersistError, SchemaEntry, SearchResult, SettingInfo, Validator,
    GlobalSettings, ProjectSettings,
    NS_EDITOR, NS_PROJECT,
    register_default_settings,
};
