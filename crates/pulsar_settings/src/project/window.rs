use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "window";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Window", "Game window presentation settings")
        .setting("title",
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
                .validator(Validator::int_range(0, 7)))
        .setting("target_fps",
            SchemaEntry::new("Target frame rate cap for the game window (0 = uncapped)", 60_i64)
                .label("Target FPS").page("Window")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(360.0), step: Some(1.0) })
                .validator(Validator::int_range(0, 360)))
        .setting("vsync_mode",
            SchemaEntry::new("Vertical synchronization mode", "adaptive")
                .label("V-Sync").page("Window")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Off", "off"),
                    DropdownOption::new("On (hard sync)", "on"),
                    DropdownOption::new("Adaptive", "adaptive"),
                    DropdownOption::new("Mailbox (low latency)", "mailbox"),
                ]})
                .validator(Validator::string_one_of(["off", "on", "adaptive", "mailbox"])))
        .setting("hdr_enabled",
            SchemaEntry::new("Enable HDR output on supported displays", false)
                .label("HDR Output").page("Window")
                .field_type(FieldType::Checkbox))
        .setting("hdr_nits_max",
            SchemaEntry::new("Peak brightness for HDR output in nits", 1000.0_f64)
                .label("HDR Peak Brightness (nits)").page("Window")
                .field_type(FieldType::NumberInput { min: Some(400.0), max: Some(10000.0), step: Some(100.0) })
                .validator(Validator::float_range(400.0, 10000.0)))
        .setting("ui_scale",
            SchemaEntry::new("Global HUD and UI scale factor", 1.0_f64)
                .label("UI Scale").page("Window")
                .field_type(FieldType::Slider { min: 0.5, max: 3.0, step: 0.05 })
                .validator(Validator::float_range(0.5, 3.0)))
        .setting("render_resolution_scale",
            SchemaEntry::new("Internal render resolution scale relative to the window size (1.0 = native)", 1.0_f64)
                .label("Render Scale").page("Window")
                .field_type(FieldType::Slider { min: 0.25, max: 2.0, step: 0.05 })
                .validator(Validator::float_range(0.25, 2.0)))
        .setting("dynamic_resolution",
            SchemaEntry::new("Dynamically adjust render scale to maintain the target frame rate", false)
                .label("Dynamic Resolution").page("Window")
                .field_type(FieldType::Checkbox))
        .setting("dynamic_resolution_target_fps",
            SchemaEntry::new("FPS target that dynamic resolution scaling tries to maintain", 60_i64)
                .label("Dynamic Resolution Target FPS").page("Window")
                .field_type(FieldType::NumberInput { min: Some(20.0), max: Some(240.0), step: Some(1.0) })
                .validator(Validator::int_range(20, 240)))
        .setting("window_title",
            SchemaEntry::new("Custom window title override (empty = use project name)", "")
                .label("Window Title Override").page("Window")
                .field_type(FieldType::TextInput { placeholder: Some("My Game".into()), multiline: false }))
        .setting("always_on_top",
            SchemaEntry::new("Keep the game window above all other windows", false)
                .label("Always on Top").page("Window")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
