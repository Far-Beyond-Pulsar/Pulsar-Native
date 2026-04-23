use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "project";
pub const OWNER: &str = "paths";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Paths", "Directory paths for project resources")
        .setting(
            "assets",
            SchemaEntry::new("Root directory for all game assets", "assets/")
                .label("Assets Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "scripts",
            SchemaEntry::new("Directory for game scripts (Lua / WASM)", "scripts/")
                .label("Scripts Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "shaders",
            SchemaEntry::new("Directory for custom shader source files", "shaders/")
                .label("Shaders Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "plugins",
            SchemaEntry::new("Directory for project-local plugins", "plugins/")
                .label("Plugins Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "savegames",
            SchemaEntry::new("Directory for save game files", "saves/")
                .label("Savegames Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "config",
            SchemaEntry::new("Directory for runtime configuration overrides", "config/")
                .label("Config Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "logs",
            SchemaEntry::new("Directory where log files are written", "logs/")
                .label("Logs Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "cache",
            SchemaEntry::new("Directory for shader / asset caches", ".cache/")
                .label("Cache Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "screenshots",
            SchemaEntry::new(
                "Directory where in-game screenshots are saved",
                "screenshots/",
            )
            .label("Screenshots Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "intermediate",
            SchemaEntry::new(
                "Directory for generated intermediate build artifacts",
                ".intermediate/",
            )
            .label("Intermediate Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "derived_data",
            SchemaEntry::new(
                "Directory for derived/cooked asset data cached between builds",
                ".ddc/",
            )
            .label("Derived Data Cache")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "binaries",
            SchemaEntry::new(
                "Output directory for compiled game and editor binaries",
                "bin/",
            )
            .label("Binaries Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "documentation",
            SchemaEntry::new(
                "Directory containing project documentation and in-editor help files",
                "docs/",
            )
            .label("Documentation Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "localization",
            SchemaEntry::new(
                "Root directory for string tables and localization assets",
                "assets/localization/",
            )
            .label("Localization Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "dlc",
            SchemaEntry::new(
                "Root directory for downloadable content (DLC) packages",
                "dlc/",
            )
            .label("DLC Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "mods",
            SchemaEntry::new("Directory where player mod packages are installed", "mods/")
                .label("Mods Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "analytics",
            SchemaEntry::new(
                "Directory for local analytics events queued before upload",
                ".analytics/",
            )
            .label("Analytics Queue Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "crash_reports",
            SchemaEntry::new("Directory where crash dump files are written", "crashes/")
                .label("Crash Reports Directory")
                .page("Paths")
                .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "thumbnails",
            SchemaEntry::new(
                "Cache directory for asset thumbnail images shown in the browser",
                ".thumbnails/",
            )
            .label("Thumbnails Cache")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        )
        .setting(
            "video_captures",
            SchemaEntry::new(
                "Directory where in-game video recordings are saved",
                "captures/",
            )
            .label("Video Captures Directory")
            .page("Paths")
            .field_type(FieldType::PathSelector { directory: true }),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
