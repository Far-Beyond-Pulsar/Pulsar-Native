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
                .validator(Validator::string_one_of(["24h", "12h"])))
        .setting("first_day_of_week",
            SchemaEntry::new("First day of the week in calendar widgets", "monday")
                .label("First Day of Week").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Monday", "monday"),
                    DropdownOption::new("Sunday", "sunday"),
                    DropdownOption::new("Saturday", "saturday"),
                ]})
                .validator(Validator::string_one_of(["monday", "sunday", "saturday"])))
        .setting("measurement_system",
            SchemaEntry::new("Unit system for distances shown in the editor", "metric")
                .label("Measurement System").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Metric (m, km)", "metric"),
                    DropdownOption::new("Imperial (ft, mi)", "imperial"),
                ]})
                .validator(Validator::string_one_of(["metric", "imperial"])))
        .setting("number_format",
            SchemaEntry::new("Thousands separator and decimal point style", "en")
                .label("Number Format").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("1,234.56 (English)", "en"),
                    DropdownOption::new("1.234,56 (European)", "eu"),
                    DropdownOption::new("1 234,56 (French)", "fr"),
                    DropdownOption::new("1234.56 (No separator)", "none"),
                ]}))
        .setting("spellcheck_enabled",
            SchemaEntry::new("Enable spell-check in text input fields and the code editor", true)
                .label("Spell Check").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("spellcheck_language",
            SchemaEntry::new("Language code for the spell-check dictionary (empty = follow editor language)", "")
                .label("Spell Check Language").page("Localization")
                .field_type(FieldType::TextInput { placeholder: Some("en-US".into()), multiline: false }))
        .setting("spellcheck_personal_dict",
            SchemaEntry::new("Path to a personal word list file for custom words", "")
                .label("Personal Dictionary").page("Localization")
                .field_type(FieldType::TextInput { placeholder: Some("config/dictionary.txt".into()), multiline: false }))
        .setting("spellcheck_in_comments",
            SchemaEntry::new("Run spell-check inside code comments", true)
                .label("Spell Check in Comments").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("spellcheck_in_strings",
            SchemaEntry::new("Run spell-check inside string literals", false)
                .label("Spell Check in Strings").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("rtl_ui_support",
            SchemaEntry::new("Enable right-to-left layout mirroring for RTL languages", false)
                .label("RTL UI Support").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("pseudo_locale",
            SchemaEntry::new("Replace all UI strings with decorated pseudo-locale text (for layout QA)", false)
                .label("Pseudo-Locale").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("locale_override",
            SchemaEntry::new("Force a specific locale code, overriding the editor language (empty = disabled)", "")
                .label("Locale Override").page("Localization")
                .field_type(FieldType::TextInput { placeholder: Some("en-US".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
