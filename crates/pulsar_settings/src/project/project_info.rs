use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "project";
pub const OWNER: &str = "project";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Project", "Core project identity and metadata")
        .setting("name",
            SchemaEntry::new("Name of the game/application", "MyGame")
                .label("Project Name").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("MyGame".into()), multiline: false }))
        .setting("version",
            SchemaEntry::new("Semantic version of the project", "0.1.0")
                .label("Version").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("0.1.0".into()), multiline: false }))
        .setting("author",
            SchemaEntry::new("Primary author or main developer name", "")
                .label("Author").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("Your Name".into()), multiline: false }))
        .setting("company",
            SchemaEntry::new("Studio or company name shown in credits and metadata", "")
                .label("Company / Studio").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("Your Studio".into()), multiline: false }))
        .setting("description",
            SchemaEntry::new("Short description of the game shown in metadata", "")
                .label("Description").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("A great game.".into()), multiline: true }))
        .setting("homepage_url",
            SchemaEntry::new("Project website or store page URL", "")
                .label("Homepage URL").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("https://example.com".into()), multiline: false }))
        .setting("support_url",
            SchemaEntry::new("Support or bug report URL", "")
                .label("Support URL").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("https://example.com/support".into()), multiline: false }))
        .setting("license",
            SchemaEntry::new("Software license for this project", "MIT")
                .label("License").page("Project")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::same("MIT"),
                    DropdownOption::same("Apache 2.0"),
                    DropdownOption::same("GPL-3.0"),
                    DropdownOption::same("LGPL-3.0"),
                    DropdownOption::same("MPL-2.0"),
                    DropdownOption::same("Proprietary"),
                    DropdownOption::new("Other", "other"),
                ]}))
        .setting("tags",
            SchemaEntry::new("Comma-separated genre/category tags", "")
                .label("Tags").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("action, rpg, open-world".into()), multiline: false }))
        .setting("engine_version",
            SchemaEntry::new("Minimum engine version required to open this project", "1.0.0")
                .label("Min Engine Version").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("1.0.0".into()), multiline: false }))
        .setting("default_map",
            SchemaEntry::new("Scene/map to load when the project starts", "scenes/default_level.json")
                .label("Default Map").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("scenes/default_level.json".into()), multiline: false }))
        .setting("engine_version_min",
            SchemaEntry::new("Minimum Pulsar engine version this project requires", "0.1.0")
                .label("Min Engine Version").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("0.1.0".into()), multiline: false }))
        .setting("copyright",
            SchemaEntry::new("Copyright notice embedded in build metadata", "")
                .label("Copyright").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("Copyright 2025 My Studio".into()), multiline: false }))
        .setting("license_spdx",
            SchemaEntry::new("SPDX license identifier for the project (e.g. MIT, Apache-2.0)", "")
                .label("License (SPDX)").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("MIT".into()), multiline: false }))
        .setting("support_url",
            SchemaEntry::new("URL for player support portal or bug tracker", "")
                .label("Support URL").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("https://support.example.com".into()), multiline: false }))
        .setting("privacy_policy_url",
            SchemaEntry::new("URL of the project privacy policy shown at first launch", "")
                .label("Privacy Policy URL").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("https://example.com/privacy".into()), multiline: false }))
        .setting("analytics_enabled",
            SchemaEntry::new("Allow the game to collect and send anonymous gameplay analytics", false)
                .label("Analytics").page("Project")
                .field_type(FieldType::Checkbox))
        .setting("crash_reporting_enabled",
            SchemaEntry::new("Automatically capture and upload crash reports for debugging", true)
                .label("Crash Reporting").page("Project")
                .field_type(FieldType::Checkbox))
        .setting("crash_report_url",
            SchemaEntry::new("Endpoint URL where crash dumps are uploaded", "")
                .label("Crash Report URL").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("https://sentry.io/api/...".into()), multiline: false }))
        .setting("content_rating",
            SchemaEntry::new("Official content rating for distribution platforms", "unrated")
                .label("Content Rating").page("Project")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Unrated", "unrated"),
                    DropdownOption::new("ESRB E", "esrb_e"),
                    DropdownOption::new("ESRB E10+", "esrb_e10"),
                    DropdownOption::new("ESRB T", "esrb_t"),
                    DropdownOption::new("ESRB M", "esrb_m"),
                    DropdownOption::new("PEGI 3", "pegi3"),
                    DropdownOption::new("PEGI 7", "pegi7"),
                    DropdownOption::new("PEGI 12", "pegi12"),
                    DropdownOption::new("PEGI 16", "pegi16"),
                    DropdownOption::new("PEGI 18", "pegi18"),
                ]}))
        .setting("genre_tags",
            SchemaEntry::new("Comma-separated genre tags for marketplace discovery", "")
                .label("Genre Tags").page("Project")
                .field_type(FieldType::TextInput { placeholder: Some("action,rpg,open-world".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
