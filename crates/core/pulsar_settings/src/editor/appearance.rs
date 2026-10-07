use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "appearance";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Appearance", "Editor-wide visual preferences")
        .setting(
            "font_size",
            SchemaEntry::new("Base UI font size", 14_i64)
                .label("Font Size")
                .page("Appearance")
                .field_type(FieldType::NumberInput {
                    min: Some(10.0),
                    max: Some(24.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(10, 24)),
        )
        .setting(
            "radius",
            SchemaEntry::new("Corner radius", 6_i64)
                .label("Corner Radius")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: [0, 4, 6, 8]
                        .into_iter()
                        .map(|v| DropdownOption::new(format!("{v}px"), v.to_string()))
                        .collect(),
                }),
        )
        .setting(
            "scrollbar_show",
            SchemaEntry::new("When scrollbars are visible", "scrolling")
                .label("Scrollbars")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("While scrolling", "scrolling"),
                        DropdownOption::new("On hover", "hover"),
                        DropdownOption::new("Always", "always"),
                    ],
                }),
        );
    let _ = cfg.register(NS, OWNER, schema);
}
