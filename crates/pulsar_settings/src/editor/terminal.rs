use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "terminal";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Terminal", "Integrated terminal emulator settings")
        .setting("shell",
            SchemaEntry::new("Default shell executable for the integrated terminal", "")
                .label("Shell").page("Terminal")
                .field_type(FieldType::TextInput { placeholder: Some("/bin/bash".into()), multiline: false }))
        .setting("font_family",
            SchemaEntry::new("Font family for the terminal", "JetBrains Mono")
                .label("Terminal Font").page("Terminal")
                .field_type(FieldType::TextInput { placeholder: Some("JetBrains Mono".into()), multiline: false }))
        .setting("font_size",
            SchemaEntry::new("Font size for the terminal (pt)", 13_i64)
                .label("Terminal Font Size").page("Terminal")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(32.0), step: Some(1.0) })
                .validator(Validator::int_range(8, 32)))
        .setting("line_height",
            SchemaEntry::new("Line height multiplier for terminal text", 1.2_f64)
                .label("Terminal Line Height").page("Terminal")
                .field_type(FieldType::Slider { min: 1.0, max: 2.0, step: 0.05 })
                .validator(Validator::float_range(1.0, 2.0)))
        .setting("scrollback_lines",
            SchemaEntry::new("Number of scrollback lines to keep in terminal history", 10000_i64)
                .label("Scrollback Lines").page("Terminal")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(1_000_000.0), step: Some(100.0) })
                .validator(Validator::int_range(100, 1_000_000)))
        .setting("cursor_style",
            SchemaEntry::new("Terminal cursor appearance", "block")
                .label("Cursor Style").page("Terminal")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Block", "block"),
                    DropdownOption::new("Underline", "underline"),
                    DropdownOption::new("Bar", "bar"),
                ]})
                .validator(Validator::string_one_of(["block", "underline", "bar"])))
        .setting("cursor_blink",
            SchemaEntry::new("Enable cursor blinking animation", true)
                .label("Cursor Blink").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("bell_enabled",
            SchemaEntry::new("Play a bell sound on terminal BEL character", false)
                .label("Terminal Bell").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("copy_on_select",
            SchemaEntry::new("Automatically copy selected text to clipboard", false)
                .label("Copy on Select").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("allow_ctrl_c_to_copy",
            SchemaEntry::new("Let Ctrl+C copy text when there is a selection (instead of sending SIGINT)", false)
                .label("Ctrl+C Copies").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("working_directory",
            SchemaEntry::new("Where to open new terminal sessions", "project_root")
                .label("Starting Directory").page("Terminal")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Project Root", "project_root"),
                    DropdownOption::new("Active File Directory", "file_dir"),
                    DropdownOption::new("Home Directory", "home"),
                    DropdownOption::new("Custom Path", "custom"),
                ]})
                .validator(Validator::string_one_of(["project_root", "file_dir", "home", "custom"])))
        .setting("custom_working_directory",
            SchemaEntry::new("Custom starting directory for new terminals", "")
                .label("Custom Starting Path").page("Terminal")
                .field_type(FieldType::TextInput { placeholder: Some("/path/to/dir".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
