use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "keybindings";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Keybindings", "Keyboard shortcut and keymap configuration")
        .setting("keymap_preset",
            SchemaEntry::new("Base keybinding preset", "pulsar")
                .label("Keymap Preset").page("Keybindings")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Pulsar (Default)", "pulsar"),
                    DropdownOption::new("VS Code", "vscode"),
                    DropdownOption::new("Vim / Neovim", "vim"),
                    DropdownOption::new("Emacs", "emacs"),
                    DropdownOption::new("Sublime Text", "sublime"),
                    DropdownOption::new("JetBrains IDEs", "jetbrains"),
                    DropdownOption::new("Atom", "atom"),
                    DropdownOption::new("Custom", "custom"),
                ]})
                .validator(Validator::string_one_of(["pulsar", "vscode", "vim", "emacs", "sublime", "jetbrains", "atom", "custom"])))
        .setting("custom_keymap_path",
            SchemaEntry::new("Path to a custom keybindings JSON file (applied on top of preset)", "")
                .label("Custom Keymap File").page("Keybindings")
                .field_type(FieldType::TextInput { placeholder: Some("config/keybindings.json".into()), multiline: false }))
        .setting("chord_timeout_ms",
            SchemaEntry::new("Milliseconds to wait for the next key in a multi-key chord", 500_i64)
                .label("Chord Timeout (ms)").page("Keybindings")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(3000.0), step: Some(50.0) })
                .validator(Validator::int_range(100, 3000)))
        .setting("show_chord_progress",
            SchemaEntry::new("Show a status-bar hint while a chord is in progress", true)
                .label("Show Chord Progress").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("leader_key",
            SchemaEntry::new("Key that starts leader-key sequences (leave blank to disable)", "")
                .label("Leader Key").page("Keybindings")
                .field_type(FieldType::TextInput { placeholder: Some("Space".into()), multiline: false }))
        .setting("leader_timeout_ms",
            SchemaEntry::new("Timeout after pressing the leader key before it is cancelled (ms)", 1000_i64)
                .label("Leader Timeout (ms)").page("Keybindings")
                .field_type(FieldType::NumberInput { min: Some(200.0), max: Some(5000.0), step: Some(100.0) })
                .validator(Validator::int_range(200, 5000)))
        .setting("conflict_resolution",
            SchemaEntry::new("How to handle two bindings mapped to the same key", "warn")
                .label("Conflict Resolution").page("Keybindings")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Warn (log a message)", "warn"),
                    DropdownOption::new("Last-Wins (silently override)", "last_wins"),
                    DropdownOption::new("First-Wins (ignore new binding)", "first_wins"),
                    DropdownOption::new("Error (refuse to load)", "error"),
                ]})
                .validator(Validator::string_one_of(["warn", "last_wins", "first_wins", "error"])))
        .setting("global_shortcuts_enabled",
            SchemaEntry::new("Register OS-level global shortcuts active even when the editor is not focused", false)
                .label("Global Shortcuts").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("show_keybinding_hints",
            SchemaEntry::new("Display keybinding hints next to menu items and buttons", true)
                .label("Show Keybinding Hints").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("vim_mode_enabled",
            SchemaEntry::new("Enable Vim modal editing emulation in text editors", false)
                .label("Vim Mode").page("Vim")
                .field_type(FieldType::Checkbox))
        .setting("vim_escape_key",
            SchemaEntry::new("Key sequence to return to Normal mode in Vim emulation", "Escape")
                .label("Vim Escape Key").page("Vim")
                .field_type(FieldType::TextInput { placeholder: Some("Escape".into()), multiline: false }))
        .setting("vim_ctrl_c_as_escape",
            SchemaEntry::new("Treat Ctrl+C as Escape in Vim emulation", true)
                .label("Ctrl+C as Escape").page("Vim")
                .field_type(FieldType::Checkbox))
        .setting("vim_jk_escape",
            SchemaEntry::new("Use 'jk' key sequence as an Escape alternative in insert mode", false)
                .label("jk Escape").page("Vim")
                .field_type(FieldType::Checkbox))
        .setting("vim_clipboard_register",
            SchemaEntry::new("Vim register to sync with the system clipboard", "")
                .label("Clipboard Register").page("Vim")
                .field_type(FieldType::TextInput { placeholder: Some("+".into()), multiline: false }))
        .setting("mouse_button_shortcuts",
            SchemaEntry::new("Allow mouse side-buttons (4 & 5) to be used as shortcut triggers", true)
                .label("Mouse Button Shortcuts").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("scroll_keybindings",
            SchemaEntry::new("Allow scrolling via keyboard shortcuts (e.g. Page Up/Down in editors)", true)
                .label("Scroll Keybindings").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("viewport_wasd_mode",
            SchemaEntry::new("Switch viewport navigation to WASD while right-mouse is held", true)
                .label("Viewport WASD Mode").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("shortcut_recording_timeout_ms",
            SchemaEntry::new("Time window for recording a new shortcut before the dialog auto-closes (ms)", 5000_i64)
                .label("Record Timeout (ms)").page("Keybindings")
                .field_type(FieldType::NumberInput { min: Some(1000.0), max: Some(30000.0), step: Some(500.0) })
                .validator(Validator::int_range(1000, 30000)))
        .setting("allow_modifier_only_shortcuts",
            SchemaEntry::new("Allow shortcuts that consist of modifier keys only (e.g. Shift+Ctrl)", false)
                .label("Modifier-Only Shortcuts").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("repeat_shortcuts_while_held",
            SchemaEntry::new("Fire an action repeatedly while its shortcut key is held down", false)
                .label("Repeat While Held").page("Keybindings")
                .field_type(FieldType::Checkbox))
        .setting("shortcut_repeat_delay_ms",
            SchemaEntry::new("Delay before a held shortcut starts repeating (ms)", 400_i64)
                .label("Repeat Delay (ms)").page("Keybindings")
                .field_type(FieldType::NumberInput { min: Some(50.0), max: Some(2000.0), step: Some(50.0) })
                .validator(Validator::int_range(50, 2000)))
        .setting("shortcut_repeat_rate_ms",
            SchemaEntry::new("Interval between repeated shortcut firings while held (ms)", 50_i64)
                .label("Repeat Rate (ms)").page("Keybindings")
                .field_type(FieldType::NumberInput { min: Some(10.0), max: Some(500.0), step: Some(10.0) })
                .validator(Validator::int_range(10, 500)));

    let _ = cfg.register(NS, OWNER, schema);
}
