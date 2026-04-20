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
                .field_type(FieldType::Checkbox))
        .setting("log_verbosity",
            SchemaEntry::new("Minimum log level surfaced in the editor log panel", "info")
                .label("Log Verbosity").page("Advanced")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Error", "error"),
                    DropdownOption::new("Warn", "warn"),
                    DropdownOption::new("Info", "info"),
                    DropdownOption::new("Debug", "debug"),
                    DropdownOption::new("Trace", "trace"),
                ]})
                .validator(Validator::string_one_of(["error", "warn", "info", "debug", "trace"])))
        .setting("log_file",
            SchemaEntry::new("Path to the editor log file (empty = no file logging)", "")
                .label("Log File Path").page("Advanced")
                .field_type(FieldType::TextInput { placeholder: Some(".pulsar/editor.log".into()), multiline: false }))
        .setting("log_rotate_max_size_mb",
            SchemaEntry::new("Rotate the log file when it exceeds this size in MB (0 = no rotation)", 10_i64)
                .label("Log Rotate Size (MB)").page("Advanced")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(500.0), step: Some(5.0) })
                .validator(Validator::int_range(0, 500)))
        .setting("experimental_features",
            SchemaEntry::new("Enable unstable features that are not yet ready for general use", false)
                .label("Experimental Features").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("dev_mode",
            SchemaEntry::new("Enable developer mode with extra diagnostics, raw inspector access, and internal tools", false)
                .label("Developer Mode").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("crash_reporter",
            SchemaEntry::new("Send anonymized crash reports to the Pulsar team to help fix bugs", true)
                .label("Send Crash Reports").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("telemetry",
            SchemaEntry::new("Send anonymized usage analytics to improve the editor", false)
                .label("Usage Telemetry").page("Advanced")
                .field_type(FieldType::Checkbox))
        .setting("ipc_socket_path",
            SchemaEntry::new("Path to the UNIX socket / named pipe for external tooling IPC", "")
                .label("IPC Socket Path").page("Advanced")
                .field_type(FieldType::TextInput { placeholder: Some("/tmp/pulsar.sock".into()), multiline: false }))
        .setting("http_proxy",
            SchemaEntry::new("HTTP/HTTPS proxy URL for marketplace and update network requests", "")
                .label("HTTP Proxy").page("Advanced")
                .field_type(FieldType::TextInput { placeholder: Some("http://proxy.example.com:3128".into()), multiline: false }))
        .setting("no_proxy",
            SchemaEntry::new("Comma-separated list of hostnames/CIDRs that bypass the HTTP proxy", "localhost,127.0.0.0/8")
                .label("No-Proxy Hosts").page("Advanced")
                .field_type(FieldType::TextInput { placeholder: Some("localhost,127.0.0.1".into()), multiline: false }))
        .setting("update_channel",
            SchemaEntry::new("Editor update channel", "stable")
                .label("Editor Update Channel").page("Advanced")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Stable", "stable"),
                    DropdownOption::new("Beta", "beta"),
                    DropdownOption::new("Nightly", "nightly"),
                ]})
                .validator(Validator::string_one_of(["stable", "beta", "nightly"])))
        .setting("auto_update_editor",
            SchemaEntry::new("Automatically download and install editor updates in the background", true)
                .label("Auto-Update Editor").page("Advanced")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
