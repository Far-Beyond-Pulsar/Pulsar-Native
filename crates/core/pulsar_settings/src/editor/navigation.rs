use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry};

pub const NS: &str = "editor";
pub const OWNER: &str = "navigation";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new(
        "Navigation",
        "How editor tabs and project content are reached",
    )
    .setting(
        "unified_sidebar",
        SchemaEntry::new(
            "Replace the tab bar with a left sidebar that lists open editors and the project's folders (preview)",
            false,
        )
        .label("Unified Left Sidebar")
        .page("Navigation")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "sidebar_pinned",
        SchemaEntry::new(
            "Keep the sidebar open beside the editor instead of opening it on hover",
            false,
        )
        .label("Keep Sidebar Open")
        .page("Navigation")
        .field_type(FieldType::Checkbox),
    );
    let _ = cfg.register(NS, OWNER, schema);
}
