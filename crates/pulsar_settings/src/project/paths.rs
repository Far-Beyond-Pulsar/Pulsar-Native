use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "project";
pub const OWNER: &str = "paths";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Paths", "Directory paths for project resources")
        .setting("assets",
            SchemaEntry::new("Root directory for all game assets", "assets/")
                .label("Assets Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("scripts",
            SchemaEntry::new("Directory for game scripts (Lua / WASM)", "scripts/")
                .label("Scripts Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("shaders",
            SchemaEntry::new("Directory for custom shader source files", "shaders/")
                .label("Shaders Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("plugins",
            SchemaEntry::new("Directory for project-local plugins", "plugins/")
                .label("Plugins Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("savegames",
            SchemaEntry::new("Directory for save game files", "saves/")
                .label("Savegames Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("config",
            SchemaEntry::new("Directory for runtime configuration overrides", "config/")
                .label("Config Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("logs",
            SchemaEntry::new("Directory where log files are written", "logs/")
                .label("Logs Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("cache",
            SchemaEntry::new("Directory for shader / asset caches", ".cache/")
                .label("Cache Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("screenshots",
            SchemaEntry::new("Directory where in-game screenshots are saved", "screenshots/")
                .label("Screenshots Directory").page("Paths")
                .field_type(FieldType::PathSelector { directory: true }));

    let _ = cfg.register(NS, OWNER, schema);
}
