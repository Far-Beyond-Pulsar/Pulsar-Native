use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry};
use pulsar_config::Validator;

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
                .field_type(FieldType::Checkbox))
        .setting("supported_locales",
            SchemaEntry::new("Comma-separated list of locale codes the game officially supports", "en")
                .label("Supported Locales").page("Localization")
                .field_type(FieldType::TextInput { placeholder: Some("en,fr,de,es,ja".into()), multiline: false }))
        .setting("native_locale",
            SchemaEntry::new("The locale in which source text strings are originally written", "en")
                .label("Native / Source Locale").page("Localization")
                .field_type(FieldType::TextInput { placeholder: Some("en".into()), multiline: false }))
        .setting("translation_file_format",
            SchemaEntry::new("Format of translation files on disk", "json")
                .label("Translation File Format").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("JSON", "json"),
                    DropdownOption::new("TOML", "toml"),
                    DropdownOption::new("PO/POT (GNU gettext)", "po"),
                    DropdownOption::new("XLIFF 1.2", "xliff"),
                    DropdownOption::new("CSV", "csv"),
                ]})
                .validator(Validator::string_one_of(["json", "toml", "po", "xliff", "csv"])))
        .setting("export_path",
            SchemaEntry::new("Directory where translation export files are written for translators", "localization/export/")
                .label("Export Path").page("Localization")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("import_path",
            SchemaEntry::new("Directory from which completed translation files are imported", "localization/import/")
                .label("Import Path").page("Localization")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("compiled_strings_dir",
            SchemaEntry::new("Directory for compiled binary string tables used at runtime", "assets/strings/")
                .label("Compiled Strings Directory").page("Localization")
                .field_type(FieldType::PathSelector { directory: true }))
        .setting("audio_localization",
            SchemaEntry::new("Enable locale-specific audio asset variants (e.g. dubbed voice lines)", false)
                .label("Audio Localization").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("font_per_locale",
            SchemaEntry::new("Use a different font family for specific locales (defined in font config)", false)
                .label("Per-Locale Fonts").page("Localization")
                .field_type(FieldType::Checkbox))
        .setting("missing_string_policy",
            SchemaEntry::new("What to show when a translated string is missing", "fallback")
                .label("Missing String Policy").page("Localization")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Use Fallback Locale", "fallback"),
                    DropdownOption::new("Show Key", "key"),
                    DropdownOption::new("Show Empty String", "empty"),
                    DropdownOption::new("Show ??? placeholder", "placeholder"),
                ]})
                .validator(Validator::string_one_of(["fallback", "key", "empty", "placeholder"])))
        .setting("number_plural_rules",
            SchemaEntry::new("Plural form rules file path for locale-specific noun/verb plurality", "")
                .label("Plural Rules File").page("Localization")
                .field_type(FieldType::TextInput { placeholder: Some("config/plural_rules.toml".into()), multiline: false }))
        .setting("locale_change_requires_restart",
            SchemaEntry::new("Require the player to restart before a locale change takes full effect", false)
                .label("Restart Required on Locale Change").page("Localization")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
