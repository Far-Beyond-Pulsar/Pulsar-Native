use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "window";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Window", "Game window presentation settings")
            SchemaEntry::new("Title shown in the OS window title bar", "My Game")
                .label("Window Title").page("Window")
                .field_type(FieldType::TextInput { placeholder: Some("My Game".into()), multiline: false }))
        .setting("width",
            SchemaEntry::new("Initial window width in pixels", 1280_i64)
                .label("Width").page("Window")
                .field_type(FieldType::NumberInput { min: Some(320.0), max: Some(7680.0), step: Some(1.0) })
                .validator(Validator::int_range(320, 7680)))
        .setting("height",
            SchemaEntry::new("Initial window height in pixels", 720_i64)
                .label("Height").page("Window")
                .field_type(FieldType::NumberInput { min: Some(240.0), max: Some(4320.0), step: Some(1.0) })
                .validator(Validator::int_range(240, 4320)))
        .setting("fullscreen",
            SchemaEntry::new("Start the game in fullscreen mode", false)
                .label("Fullscreen").page("Window")
                .field_type(FieldType::Checkbox))
        .setting("fullscreen_mode",
            SchemaEntry::new("Type of fullscreen to use", "borderless")
                .label("Fullscreen Mode").page("Window")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Exclusive", "exclusive"),
                    DropdownOption::new("Borderless", "borderless"),
                ]})
                .validator(Validator::string_one_of(["exclusive", "borderless"])))
        .setting("vsync",
            SchemaEntry::new("Enable vertical sync to prevent tearing", true)
                .label("V-Sync").page("Window")
                .field_type(FieldType::Checkbox))
        .setting("resizable",
            SchemaEntry::new("Allow the player to resize the window", true)
                .label("Resizable").page("Window")
                .field_type(FieldType::Checkbox))
        .setting("min_width",
            SchemaEntry::new("Minimum allowed window width (px)", 640_i64)
                .label("Min Width").page("Window")
                .field_type(FieldType::NumberInput { min: Some(320.0), max: Some(3840.0), step: Some(1.0) })
                .validator(Validator::int_range(320, 3840)))
        .setting("min_height",
            SchemaEntry::new("Minimum allowed window height (px)", 360_i64)
                .label("Min Height").page("Window")
                .field_type(FieldType::NumberInput { min: Some(240.0), max: Some(2160.0), step: Some(1.0) })
                .validator(Validator::int_range(240, 2160)))
        .setting("borderless",
            SchemaEntry::new("Show the window without an OS title bar or borders", false)
                .label("Borderless").page("Window")
                .field_type(FieldType::Checkbox))
        .setting("always_on_top",
            SchemaEntry::new("Force the game window to always stay on top of other windows", false)
                .label("Always On Top").page("Window")
                .field_type(FieldType::Checkbox))
        .setting("icon",
            SchemaEntry::new("Path to the window icon image (.png or .ico)", "assets/icon.png")
                .label("Window Icon").page("Window")
                .field_type(FieldType::TextInput { placeholder: Some("assets/icon.png".into()), multiline: false }))
        .setting("display_index",
            SchemaEntry::new("Which monitor to open the window on (0 = primary)", 0_i64)
                .label("Display Index").page("Window")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(7.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 7)));

    let _ = cfg.register(NS, OWNER, schema);
}
