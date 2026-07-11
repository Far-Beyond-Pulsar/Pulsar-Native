use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "extensions";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Extensions", "Plugin and extension marketplace settings")
        .setting("auto_update_extensions",
            SchemaEntry::new("Automatically update installed extensions to newer versions", true)
                .label("Auto-Update Extensions").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("update_channel",
            SchemaEntry::new("Extension release channel to receive updates from", "stable")
                .label("Update Channel").page("Extensions")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Stable", "stable"),
                    DropdownOption::new("Pre-Release", "pre_release"),
                    DropdownOption::new("Nightly", "nightly"),
                ]})
                .validator(Validator::string_one_of(["stable", "pre_release", "nightly"])))
        .setting("check_update_interval_hours",
            SchemaEntry::new("Hours between automatic extension update checks", 24_i64)
                .label("Update Check Interval (h)").page("Extensions")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(168.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 168)))
        .setting("notify_on_update",
            SchemaEntry::new("Show a notification when extension updates are available", true)
                .label("Notify on Updates").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("auto_restart_after_update",
            SchemaEntry::new("Automatically restart the editor after extensions are updated", false)
                .label("Auto-Restart After Update").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("extension_install_dir",
            SchemaEntry::new("Directory where extensions are installed", "extensions/")
                .label("Extension Directory").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("extensions/".into()), multiline: false }))
        .setting("user_extension_dir",
            SchemaEntry::new("Per-user extensions directory (merged with the project extension dir)", "~/.pulsar/extensions/")
                .label("User Extension Directory").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("~/.pulsar/extensions/".into()), multiline: false }))
        .setting("allow_unsigned_extensions",
            SchemaEntry::new("Load extensions that have not been signed by a trusted publisher", false)
                .label("Allow Unsigned Extensions").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("verify_checksums",
            SchemaEntry::new("Verify SHA-256 checksums of downloaded extension archives before installing", true)
                .label("Verify Checksums").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("trusted_publishers",
            SchemaEntry::new("Comma-separated list of trusted extension publisher IDs (extensions from these skip signing checks)", "")
                .label("Trusted Publishers").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("far-beyond-dev,my-studio".into()), multiline: false }))
        .setting("marketplace_url",
            SchemaEntry::new("Base URL of the extension marketplace API", "https://marketplace.pulsar-engine.dev")
                .label("Marketplace URL").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("https://marketplace.pulsar-engine.dev".into()), multiline: false }))
        .setting("additional_registries",
            SchemaEntry::new("Additional extension registry URLs (newline-separated)", "")
                .label("Additional Registries").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("https://my-registry.example.com".into()), multiline: true }))
        .setting("show_recommendations",
            SchemaEntry::new("Show extension recommendations based on open file types and project structure", true)
                .label("Show Recommendations").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("extension_sync",
            SchemaEntry::new("Sync installed extensions list to cloud profile", false)
                .label("Sync Extensions").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("extension_host_memory_mb",
            SchemaEntry::new("Maximum memory an extension host process may use (MB)", 512_i64)
                .label("Extension Host Memory (MB)").page("Extensions")
                .field_type(FieldType::NumberInput { min: Some(64.0), max: Some(4096.0), step: Some(64.0) })
                .validator(Validator::int_range(64, 4096)))
        .setting("extension_host_timeout_ms",
            SchemaEntry::new("Time before an unresponsive extension host is killed (ms)", 5000_i64)
                .label("Extension Host Timeout (ms)").page("Extensions")
                .field_type(FieldType::NumberInput { min: Some(1000.0), max: Some(30000.0), step: Some(500.0) })
                .validator(Validator::int_range(1000, 30000)))
        .setting("max_restart_attempts",
            SchemaEntry::new("How many times a crashed extension host is restarted before being disabled", 3_i64)
                .label("Max Restart Attempts").page("Extensions")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(10.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 10)))
        .setting("extension_logs_dir",
            SchemaEntry::new("Directory where extension host log files are written", "logs/extensions/")
                .label("Extension Logs Directory").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("logs/extensions/".into()), multiline: false }))
        .setting("disable_builtin_extensions",
            SchemaEntry::new("Comma-separated list of built-in extension IDs to forcibly disable", "")
                .label("Disabled Built-in Extensions").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("builtin.git,builtin.json".into()), multiline: false }))
        .setting("telemetry_for_extensions",
            SchemaEntry::new("Allow extensions to send anonymous usage telemetry", false)
                .label("Extension Telemetry").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("show_extension_badge_count",
            SchemaEntry::new("Show update/error badge count on the Extensions icon in the activity bar", true)
                .label("Show Badge Count").page("Extensions")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
