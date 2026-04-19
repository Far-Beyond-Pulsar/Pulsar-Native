use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "advanced";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Advanced", "Low-level engine and editor settings")
        .setting("debug_logging",
            SchemaEntry::new("Enable verbose debug logging to the output log", false)
                .label("Debug Logging").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("log_level",
            SchemaEntry::new("Minimum log level to display in the output panel", "info")
                .label("Log Level").page("Advanced")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Trace", "trace"),
                    DropdownOption::new("Debug", "debug"),
                    DropdownOption::new("Info", "info"),
                    DropdownOption::new("Warning", "warn"),
                    DropdownOption::new("Error", "error"),
                ]})
                .validator(Validator::string_one_of(["trace", "debug", "info", "warn", "error"])))
        .setting("log_to_file",
            SchemaEntry::new("Write engine logs to a file in the project logs/ directory", true)
                .label("Log to File").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("max_log_file_size_mb",
            SchemaEntry::new("Maximum size of a single log file before rotation (MB)", 50_i64)
                .label("Max Log File (MB)").page("Advanced")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(1024.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 1024)))
        .setting("max_log_files",
            SchemaEntry::new("Maximum number of rotated log files to keep", 5_i64)
                .label("Max Log Files").page("Advanced")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(50.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 50)))
        .setting("experimental_features",
            SchemaEntry::new("Enable experimental in-development features (may be unstable)", false)
                .label("Experimental Features").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("telemetry",
            SchemaEntry::new("Send anonymous crash reports and usage statistics to improve the engine", false)
                .label("Anonymous Telemetry").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("crash_reporter",
            SchemaEntry::new("Automatically capture and upload crash dumps", true)
                .label("Crash Reporter").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("hot_reload_scripts",
            SchemaEntry::new("Reload Lua/WASM scripts without restarting play mode", true)
                .label("Hot Reload Scripts").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("native_file_dialogs",
            SchemaEntry::new("Use the OS native file open/save dialogs", true)
                .label("Native File Dialogs").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("gpu_validation",
            SchemaEntry::new("Enable GPU API validation layer (Vulkan/DX12 only — large overhead)", false)
                .label("GPU Validation").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("shader_debug_info",
            SchemaEntry::new("Embed debug information into compiled shaders", false)
                .label("Shader Debug Info").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("allow_unsafe_plugins",
            SchemaEntry::new("Load plugins that have not been signed or verified", false)
                .label("Allow Unsigned Plugins").page("Advanced")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
