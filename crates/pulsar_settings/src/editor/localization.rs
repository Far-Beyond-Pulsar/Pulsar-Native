use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "localization";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Localization (Editor)", "Editor language and regional settings")
        .setting("editor_language",
            SchemaEntry::new("Display language for the editor interface", "en")
                .label("Editor Language").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("English", "en"),
                    DropdownOption::new("French", "fr"),
                    DropdownOption::new("German", "de"),
                    DropdownOption::new("Spanish", "es"),
                    DropdownOption::new("Portuguese", "pt"),
                    DropdownOption::new("Japanese", "ja"),
                    DropdownOption::new("Korean", "ko"),
                    DropdownOption::new("Simplified Chinese", "zh-Hans"),
                    DropdownOption::new("Traditional Chinese", "zh-Hant"),
                    DropdownOption::new("Russian", "ru"),
                    DropdownOption::new("Italian", "it"),
                    DropdownOption::new("Polish", "pl"),
                ]}))
        .setting("use_system_locale",
            SchemaEntry::new("Inherit date, time, and number formats from the OS locale", true)
                .label("Use System Locale").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("date_format",
            SchemaEntry::new("Date display format used in the editor", "YYYY-MM-DD")
                .label("Date Format").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("ISO 8601 (YYYY-MM-DD)", "YYYY-MM-DD"),
                    DropdownOption::new("US (MM/DD/YYYY)", "MM/DD/YYYY"),
                    DropdownOption::new("EU (DD.MM.YYYY)", "DD.MM.YYYY"),
                ]}))
        .setting("time_format",
            SchemaEntry::new("24-hour vs 12-hour time display", "24h")
                .label("Time Format").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("24-hour", "24h"),
                    DropdownOption::new("12-hour (AM/PM)", "12h"),
                ]})
                .validator(Validator::string_one_of(["24h", "12h"])));

    let _ = cfg.register(NS, OWNER, schema);
}
