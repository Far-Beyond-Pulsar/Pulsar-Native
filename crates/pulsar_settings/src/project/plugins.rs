use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "project";
pub const OWNER: &str = "plugins";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Plugins", "Runtime plugin loading and management")
        .setting("allow_third_party",
            SchemaEntry::new("Allow loading plugins from third-party sources (not bundled with engine)", true)
                .label("Allow Third-Party Plugins").page("Plugins")
                .field_type(FieldType::Checkbox))
        .setting("require_signed",
            SchemaEntry::new("Only load plugins that have a valid digital signature", false)
                .label("Require Signed Plugins").page("Plugins")
                .field_type(FieldType::Checkbox))
        .setting("search_paths",
            SchemaEntry::new("Additional directories to search for plugins (semicolon-separated)", "")
                .label("Plugin Search Paths").page("Plugins")
                .field_type(FieldType::TextInput { placeholder: Some("plugins/extra;../shared_plugins".into()), multiline: false }))
        .setting("auto_enable_new",
            SchemaEntry::new("Automatically enable newly discovered plugins", false)
                .label("Auto-Enable New Plugins").page("Plugins")
                .field_type(FieldType::Checkbox))
        .setting("plugin_sandbox",
            SchemaEntry::new("Run native plugins inside an OS process sandbox", false)
                .label("Plugin Sandbox").page("Plugins")
                .field_type(FieldType::Checkbox))
        .setting("crash_recovery",
            SchemaEntry::new("Disable a plugin automatically if it causes a crash", true)
                .label("Plugin Crash Recovery").page("Plugins")
                .field_type(FieldType::Checkbox))
        .setting("hot_reload_plugins",
            SchemaEntry::new("Reload modified plugins without restarting the editor", true)
                .label("Hot-Reload Plugins").page("Plugins")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
