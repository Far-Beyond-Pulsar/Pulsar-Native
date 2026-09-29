use pulsar_config::{ConfigManager, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "radial_menu";

/// Default quick actions, one per line: `Label | namespace::Action` (the label
/// is optional). The menu shows only the ones the focused editor handles, so
/// one list serves every editor.
pub const DEFAULT_ITEMS: &str = "\
Save | level_editor::SaveScene
Play | level_editor::PlayScene
Stop | level_editor::StopScene
Undo | level_editor::Undo
Redo | level_editor::Redo
Duplicate | level_editor::DuplicateObject
Delete | level_editor::DeleteObject
Frame Selected | level_editor::FocusSelected
Toggle Grid | level_editor::ToggleGrid
Commands | pulsar_app::ToggleCommandPalette
Files | pulsar_app::ToggleFileManager";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new(
        "Radial Menu",
        "Hold Tab for a radial menu of quick actions in the focused editor",
    )
    .setting(
        "enabled",
        SchemaEntry::new("Open the radial menu while Tab is held", true)
            .label("Enable Radial Menu")
            .page("Radial Menu")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "hold_delay_ms",
        SchemaEntry::new(
            "How long Tab must be held before the menu opens (ms). A shorter press still moves focus like a normal Tab.",
            180_i64,
        )
        .label("Hold Delay (ms)")
        .page("Radial Menu")
        .field_type(FieldType::NumberInput {
            min: Some(50.0),
            max: Some(1000.0),
            step: Some(10.0),
        })
        .validator(Validator::int_range(50, 1000)),
    )
    .setting(
        "items",
        SchemaEntry::new(
            "Quick actions, one per line: `Label | namespace::Action` (label optional). Only actions the focused editor handles are shown; at most 12.",
            DEFAULT_ITEMS,
        )
        .label("Menu Items")
        .page("Radial Menu")
        .field_type(FieldType::TextInput {
            placeholder: Some("Save | level_editor::SaveScene".into()),
            multiline: true,
        }),
    );

    let _ = cfg.register(NS, OWNER, schema);
}
