use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "extensions";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Extensions", "Plugin and extension marketplace settings")
        .setting("auto_update_extensions",
            SchemaEntry::new("Automatically update installed extensions to newer versions", true)
                .label("Auto-Update Extensions").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("check_update_interval_hours",
            SchemaEntry::new("Hours between automatic extension update checks", 24_i64)
                .label("Update Check Interval (h)").page("Extensions")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(168.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 168)))
        .setting("extension_install_dir",
            SchemaEntry::new("Directory where extensions are installed", "extensions/")
                .label("Extension Directory").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("extensions/".into()), multiline: false }))
        .setting("allow_unsigned_extensions",
            SchemaEntry::new("Load extensions that have not been signed by a trusted publisher", false)
                .label("Allow Unsigned Extensions").page("Extensions")
                .field_type(FieldType::Checkbox))
        .setting("marketplace_url",
            SchemaEntry::new("URL of the extension marketplace registry", "https://marketplace.pulsar-engine.dev")
                .label("Marketplace URL").page("Extensions")
                .field_type(FieldType::TextInput { placeholder: Some("https://marketplace.pulsar-engine.dev".into()), multiline: false }))
        .setting("telemetry_for_extensions",
            SchemaEntry::new("Allow extensions to collect anonymous usage data", false)
                .label("Extension Telemetry").page("Extensions")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
