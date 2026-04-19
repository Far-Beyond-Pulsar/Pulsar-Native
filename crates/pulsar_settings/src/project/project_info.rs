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
                .field_type(FieldType::TextInput { placeholder: Some("scenes/default_level.json".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
