use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "code_editor";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Code Editor", "Script editor formatting and display")
        .setting(
            "tab_width",
            SchemaEntry::new("Number of spaces represented by a tab", 4_i64)
                .label("Tab Width")
                .page("Code Editor")
                .field_type(FieldType::NumberInput {
                    min: Some(1.0),
                    max: Some(16.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(1, 16)),
        )
        .setting(
            "hard_tabs",
            SchemaEntry::new("Insert tab characters instead of spaces", false)
                .label("Hard Tabs")
                .page("Code Editor")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "line_numbers",
            SchemaEntry::new("Show line numbers beside source files", true)
                .label("Line Numbers")
                .page("Code Editor")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "minimap",
            SchemaEntry::new("Show the source minimap", true)
                .label("Minimap")
                .page("Code Editor")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "soft_wrap",
            SchemaEntry::new("Wrap long lines at the editor edge", false)
                .label("Soft Wrap")
                .page("Code Editor")
                .field_type(FieldType::Checkbox),
        );
    let _ = cfg.register(NS, OWNER, schema);
}
