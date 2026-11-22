//! The code editor's settings. The code editor plugin receives them in the
//! editor settings snapshot the host hands every editor it creates
//! (`plugin_editor_api::EditorSettingsSnapshot`), so they apply to files
//! opened after a change.

use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "code_editor";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new(
        "Code Editor",
        "Source editor display; applies to files opened after the change",
    )
    .setting(
        "tab_width",
        SchemaEntry::new(
            "Columns a tab stop spans, and how far one indent level indents",
            4_i64,
        )
        .label("Tab Width")
        .page("Display")
        .field_type(FieldType::NumberInput {
            min: Some(1.0),
            max: Some(16.0),
            step: Some(1.0),
        })
        .validator(Validator::int_range(1, 16)),
    )
    .setting(
        "minimap",
        SchemaEntry::new("Show the source minimap beside the code", true)
            .label("Minimap")
            .page("Display")
            .field_type(FieldType::Checkbox),
    );
    let _ = cfg.register(NS, OWNER, schema);
}
