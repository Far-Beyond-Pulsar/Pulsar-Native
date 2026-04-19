use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "project";
pub const OWNER: &str = "localization";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Localization", "Project internationalization and locale settings")
        .setting("default_locale",
            SchemaEntry::new("Default locale code for new players", "en")
                .label("Default Locale").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("English (en)", "en"),
                    DropdownOption::new("French (fr)", "fr"),
                    DropdownOption::new("German (de)", "de"),
                    DropdownOption::new("Spanish (es)", "es"),
                    DropdownOption::new("Portuguese – BR (pt-BR)", "pt-BR"),
                    DropdownOption::new("Japanese (ja)", "ja"),
                    DropdownOption::new("Korean (ko)", "ko"),
                    DropdownOption::new("Simplified Chinese (zh-Hans)", "zh-Hans"),
                    DropdownOption::new("Traditional Chinese (zh-Hant)", "zh-Hant"),
                    DropdownOption::new("Russian (ru)", "ru"),
                    DropdownOption::new("Italian (it)", "it"),
                    DropdownOption::new("Polish (pl)", "pl"),
                    DropdownOption::new("Turkish (tr)", "tr"),
                    DropdownOption::new("Arabic (ar)", "ar"),
                ]}))
        .setting("fallback_locale",
            SchemaEntry::new("Locale to fall back to when a string is missing in the active locale", "en")
                .label("Fallback Locale").page("Localization")
                .field_type(FieldType::TextInput { placeholder: Some("en".into()), multiline: false }))
        .setting("string_table_path",
            SchemaEntry::new("Path to the root localization string table directory", "assets/localization/")
                .label("String Table Path").page("Localization")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("auto_detect_locale",
            SchemaEntry::new("Automatically select the locale matching the OS language on first launch", true)
                .label("Auto-Detect Locale").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("enable_pseudo_localization",
            SchemaEntry::new("Replace strings with decorated pseudo-localized text for testing layout", false)
                .label("Pseudo-Localization").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("rtl_support",
            SchemaEntry::new("Enable right-to-left text layout support (Arabic, Hebrew, etc.)", false)
                .label("RTL Support").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("use_hardware_fonts",
            SchemaEntry::new("Allow the OS to provide locale-appropriate fallback fonts", true)
                .label("Use Hardware / Fallback Fonts").page("Localization")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
