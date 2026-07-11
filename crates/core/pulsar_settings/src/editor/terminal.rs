use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

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
                .field_type(FieldType::TextInput { placeholder: Some("/path/to/dir".into()), multiline: false }))
        .setting("scrollback_lines",
            SchemaEntry::new("Maximum number of lines stored in terminal scrollback buffer", 10_000_i64)
                .label("Scrollback Lines").page("Terminal")
                .field_type(FieldType::NumberInput { min: Some(100.0), max: Some(1_000_000.0), step: Some(1000.0) })
                .validator(Validator::int_range(100, 1_000_000)))
        .setting("cursor_style",
            SchemaEntry::new("Cursor style inside terminal panels", "block")
                .label("Cursor Style").page("Terminal")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Block", "block"),
                    DropdownOption::new("Underline", "underline"),
                    DropdownOption::new("Bar (|", "bar"),
                ]})
                .validator(Validator::string_one_of(["block", "underline", "bar"])))
        .setting("cursor_blink",
            SchemaEntry::new("Blink the terminal cursor", true)
                .label("Cursor Blink").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("copy_on_select",
            SchemaEntry::new("Automatically copy selected text to clipboard", false)
                .label("Copy on Select").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("terminal_bell",
            SchemaEntry::new("How to handle terminal bell (\\a escape)", "none")
                .label("Terminal Bell").page("Terminal")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Audible", "audible"),
                    DropdownOption::new("Visual Flash", "visual"),
                ]})
                .validator(Validator::string_one_of(["none", "audible", "visual"])))
        .setting("gpu_acceleration",
            SchemaEntry::new("Use GPU rendering for the terminal emulator when available", true)
                .label("GPU Acceleration").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("renderer",
            SchemaEntry::new("Terminal rendering backend", "auto")
                .label("Terminal Renderer").page("Terminal")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Auto", "auto"),
                    DropdownOption::new("OpenGL", "opengl"),
                    DropdownOption::new("Vulkan", "vulkan"),
                    DropdownOption::new("Metal", "metal"),
                    DropdownOption::new("Software", "software"),
                ]})
                .validator(Validator::string_one_of(["auto", "opengl", "vulkan", "metal", "software"])))
        .setting("word_separators",
            SchemaEntry::new("Characters treated as word boundaries for double-click selection", " ()[]{}',\";:")
                .label("Word Separators").page("Terminal")
                .field_type(FieldType::TextInput { placeholder: Some(" ()[]{}',\";".into()), multiline: false }))
        .setting("env_vars",
            SchemaEntry::new("Extra environment variables injected into terminal sessions (KEY=VAL, comma-separated)", "")
                .label("Extra Env Vars").page("Terminal")
                .field_type(FieldType::TextInput { placeholder: Some("FOO=bar,BAZ=qux".into()), multiline: false }))
        .setting("close_on_exit",
            SchemaEntry::new("Close the terminal tab automatically when the process exits", false)
                .label("Close Tab on Exit").page("Terminal")
                .field_type(FieldType::Checkbox))
        .setting("enable_osc_hyperlinks",
            SchemaEntry::new("Render OSC 8 hyperlinks as clickable links in the terminal", true)
                .label("OSC Hyperlinks").page("Terminal")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
