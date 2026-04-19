use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "keybindings";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Keybindings", "Keyboard shortcut configuration")
        .setting("keymap_preset",
            SchemaEntry::new("Base keybinding preset to use", "pulsar")
                .label("Keymap Preset").page("Keybindings")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Pulsar (Default)", "pulsar"),
                    DropdownOption::new("VS Code", "vscode"),
                    DropdownOption::new("Vim", "vim"),
                    DropdownOption::new("Emacs", "emacs"),
                    DropdownOption::new("Sublime Text", "sublime"),
                    DropdownOption::new("Custom", "custom"),
                ]})
                .validator(Validator::string_one_of(["pulsar", "vscode", "vim", "emacs", "sublime", "custom"])))
        .setting("custom_keymap_path",
            SchemaEntry::new("Path to a custom keybindings JSON file", "")
                .label("Custom Keymap File").page("Keybindings")
                .field_type(FieldType::TextInput { placeholder: Some("config/keybindings.json".into()), multiline: false }))
        .setting("chord_timeout_ms",
            SchemaEntry::new("Milliseconds to wait for a multi-key chord before giving up", 500_i64)
                .label("Chord Timeout (ms)").page("Keybindings")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(2000.0), step: Some(50.0) })
                .validator(Validator::int_range(100, 2000)));

    let _ = cfg.register(NS, OWNER, schema);
}
