use pulsar_config::Validator;
use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "project";
pub const OWNER: &str = "plugins";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Plugins", "Runtime plugin loading and management")
        .setting(
            "allow_third_party",
            SchemaEntry::new(
                "Allow loading plugins from third-party sources (not bundled with engine)",
                true,
            )
            .label("Allow Third-Party Plugins")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "require_signed",
            SchemaEntry::new(
                "Only load plugins that have a valid digital signature",
                false,
            )
            .label("Require Signed Plugins")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "search_paths",
            SchemaEntry::new(
                "Additional directories to search for plugins (semicolon-separated)",
                "",
            )
            .label("Plugin Search Paths")
            .page("Plugins")
            .field_type(FieldType::TextInput {
                placeholder: Some("plugins/extra;../shared_plugins".into()),
                multiline: false,
            }),
        )
        .setting(
            "auto_enable_new",
            SchemaEntry::new("Automatically enable newly discovered plugins", false)
                .label("Auto-Enable New Plugins")
                .page("Plugins")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "plugin_sandbox",
            SchemaEntry::new("Run native plugins inside an OS process sandbox", false)
                .label("Plugin Sandbox")
                .page("Plugins")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "crash_recovery",
            SchemaEntry::new("Disable a plugin automatically if it causes a crash", true)
                .label("Plugin Crash Recovery")
                .page("Plugins")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "hot_reload_plugins",
            SchemaEntry::new(
                "Reload modified plugins without restarting the editor",
                true,
            )
            .label("Hot-Reload Plugins")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "plugin_load_order_file",
            SchemaEntry::new(
                "Path to a file that specifies plugin load order overrides",
                "",
            )
            .label("Load Order File")
            .page("Plugins")
            .field_type(FieldType::TextInput {
                placeholder: Some("config/plugin_order.toml".into()),
                multiline: false,
            }),
        )
        .setting(
            "max_plugin_load_time_ms",
            SchemaEntry::new(
                "Maximum milliseconds a plugin may take to initialize before it is killed",
                5000_i64,
            )
            .label("Max Load Time (ms)")
            .page("Plugins")
            .field_type(FieldType::NumberInput {
                min: Some(500.0),
                max: Some(60000.0),
                step: Some(500.0),
            })
            .validator(Validator::int_range(500, 60_000)),
        )
        .setting(
            "plugin_api_version",
            SchemaEntry::new(
                "Minimum plugin API version to accept (reject older plugins)",
                "1.0.0",
            )
            .label("Min Plugin API Version")
            .page("Plugins")
            .field_type(FieldType::TextInput {
                placeholder: Some("1.0.0".into()),
                multiline: false,
            }),
        )
        .setting(
            "abi_compatibility_check",
            SchemaEntry::new(
                "Refuse to load plugins built against an incompatible ABI version",
                true,
            )
            .label("ABI Compatibility Check")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "plugin_update_channel",
            SchemaEntry::new(
                "Release channel to use when checking for plugin updates",
                "stable",
            )
            .label("Plugin Update Channel")
            .page("Plugins")
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("Stable", "stable"),
                    DropdownOption::new("Beta", "beta"),
                    DropdownOption::new("Nightly", "nightly"),
                ],
            })
            .validator(Validator::string_one_of(["stable", "beta", "nightly"])),
        )
        .setting(
            "auto_update_plugins",
            SchemaEntry::new(
                "Automatically update plugins when a new version is published",
                false,
            )
            .label("Auto-Update Plugins")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "builtin_plugins_enabled",
            SchemaEntry::new(
                "Enable all built-in first-party plugins (e.g. blueprints, navmesh)",
                true,
            )
            .label("Enable Built-in Plugins")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "community_plugins_enabled",
            SchemaEntry::new(
                "Allow installation of community-published plugins from the marketplace",
                true,
            )
            .label("Community Plugins")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "performance_monitoring",
            SchemaEntry::new(
                "Track per-plugin CPU and memory usage and surface in the profiler",
                false,
            )
            .label("Per-Plugin Profiling")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "security_audit_on_load",
            SchemaEntry::new(
                "Run a static security audit on each plugin manifest when it loads",
                true,
            )
            .label("Security Audit on Load")
            .page("Plugins")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "pre_load_native_dlls",
            SchemaEntry::new(
                "Comma-separated list of native DLLs to load before any plugins initialize",
                "",
            )
            .label("Pre-Load DLLs")
            .page("Plugins")
            .field_type(FieldType::TextInput {
                placeholder: Some("vendor.dll,helper.so".into()),
                multiline: false,
            }),
        )
        .setting(
            "excluded_plugins",
            SchemaEntry::new(
                "Comma-separated list of plugin IDs that must never be loaded in this project",
                "",
            )
            .label("Excluded Plugins")
            .page("Plugins")
            .field_type(FieldType::TextInput {
                placeholder: Some("legacy-plugin,bad-plugin".into()),
                multiline: false,
            }),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
