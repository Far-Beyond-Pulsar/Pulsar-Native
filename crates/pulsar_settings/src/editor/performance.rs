use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "performance";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Performance", "Editor performance tuning")
        .setting("max_viewport_fps",
            SchemaEntry::new("Maximum frame rate for editor viewports", "60")
                .label("Max Viewport FPS").page("Performance")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("30 FPS", "30"),
                    DropdownOption::new("60 FPS", "60"),
                    DropdownOption::new("120 FPS", "120"),
                    DropdownOption::new("144 FPS", "144"),
                    DropdownOption::new("240 FPS", "240"),
                    DropdownOption::new("Unlimited", "0"),
                ]}))
        .setting("ui_fps",
            SchemaEntry::new("Maximum frame rate for UI panels (lower = less CPU usage)", "60")
                .label("UI Frame Rate").page("Performance")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("30 FPS", "30"),
                    DropdownOption::new("60 FPS", "60"),
                    DropdownOption::new("120 FPS", "120"),
                    DropdownOption::new("Unlimited", "0"),
                ]}))
        .setting("enable_vsync",
            SchemaEntry::new("Sync editor frame rate to the monitor refresh rate", true)
                .label("Enable V-Sync").page("Performance")
                .field_type(FieldType::Checkbox))
        .setting("worker_threads",
            SchemaEntry::new("Number of background worker threads (0 = auto)", 0_i64)
                .label("Worker Threads").page("Performance")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(64.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 64)))
        .setting("asset_import_threads",
            SchemaEntry::new("Concurrent threads used when importing assets", 4_i64)
                .label("Import Threads").page("Performance")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 32)))
        .setting("shader_compilation_threads",
            SchemaEntry::new("Concurrent threads for shader compilation", 4_i64)
                .label("Shader Compile Threads").page("Performance")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 32)))
        .setting("texture_streaming_pool_mb",
            SchemaEntry::new("GPU texture streaming pool size in megabytes", 512_i64)
                .label("Texture Pool (MB)").page("Performance")
                .field_type(FieldType::NumberInput { min: Some(64.0), max: Some(16384.0), step: Some(64.0) })
                .validator(Validator::int_range(64, 16384)))
        .setting("enable_gpu_crash_diagnostics",
            SchemaEntry::new("Capture GPU breadcrumbs for crash diagnostics (small overhead)", false)
                .label("GPU Crash Diagnostics").page("Performance")
                .field_type(FieldType::Checkbox))
        .setting("suspend_when_unfocused",
            SchemaEntry::new("Throttle the editor when its window loses focus to save resources", true)
                .label("Suspend When Unfocused").page("Performance")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
