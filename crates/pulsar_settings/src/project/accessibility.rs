use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "project";
pub const OWNER: &str = "accessibility";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new(
        "Accessibility",
        "Player accessibility and inclusivity options",
    )
    .setting(
        "colorblind_mode",
        SchemaEntry::new(
            "Color vision deficiency simulation / correction mode",
            "none",
        )
        .label("Colorblind Mode")
        .page("Accessibility")
        .field_type(FieldType::Dropdown {
            options: vec![
                DropdownOption::new("None", "none"),
                DropdownOption::new("Protanopia (red-blind)", "protanopia"),
                DropdownOption::new("Deuteranopia (green-blind)", "deuteranopia"),
                DropdownOption::new("Tritanopia (blue-blind)", "tritanopia"),
                DropdownOption::new("Achromatopsia (monochrome)", "achromatopsia"),
            ],
        }),
    )
    .setting(
        "high_contrast_ui",
        SchemaEntry::new("Use a high-contrast color scheme for all in-game UI", false)
            .label("High Contrast UI")
            .page("Accessibility")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "reduced_motion",
        SchemaEntry::new(
            "Reduce or disable UI animations and camera shake for motion sensitivity",
            false,
        )
        .label("Reduced Motion")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "font_scale",
        SchemaEntry::new("Global font scale multiplier for all in-game text", 1.0_f64)
            .label("Font Scale")
            .page("Accessibility")
            .field_type(FieldType::Slider {
                min: 0.5,
                max: 3.0,
                step: 0.05,
            })
            .validator(Validator::float_range(0.5, 3.0)),
    )
    .setting(
        "subtitles_enabled",
        SchemaEntry::new(
            "Show subtitles for all dialogue and important audio cues",
            false,
        )
        .label("Subtitles")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "subtitle_font_size",
        SchemaEntry::new("Subtitle text font size (pt)", 18_i64)
            .label("Subtitle Font Size")
            .page("Accessibility")
            .field_type(FieldType::NumberInput {
                min: Some(10.0),
                max: Some(48.0),
                step: Some(1.0),
            })
            .validator(Validator::int_range(10, 48)),
    )
    .setting(
        "subtitle_background",
        SchemaEntry::new(
            "Show a semi-transparent background behind subtitle text",
            true,
        )
        .label("Subtitle Background")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "screen_reader",
        SchemaEntry::new(
            "Enable screen reader integration for menus and HUD elements",
            false,
        )
        .label("Screen Reader")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "input_remapping",
        SchemaEntry::new("Allow players to remap all input bindings in-game", true)
            .label("Allow Input Remapping")
            .page("Accessibility")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "camera_shake_intensity",
        SchemaEntry::new(
            "Multiplier for all camera shake effects (0 = disabled)",
            1.0_f64,
        )
        .label("Camera Shake Intensity")
        .page("Accessibility")
        .field_type(FieldType::Slider {
            min: 0.0,
            max: 1.0,
            step: 0.05,
        })
        .validator(Validator::float_range(0.0, 1.0)),
    )
    .setting(
        "flashing_lights",
        SchemaEntry::new(
            "Show photosensitive epilepsy warning and allow disabling flashing lights",
            true,
        )
        .label("Photosensitivity Warning")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "aim_assist_enabled",
        SchemaEntry::new(
            "Enable aim assistance for players using controllers or with motor impairments",
            false,
        )
        .label("Aim Assist")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "aim_assist_strength",
        SchemaEntry::new(
            "Strength of the aim assist magnetism (0.0 = off, 1.0 = maximum)",
            0.3_f64,
        )
        .label("Aim Assist Strength")
        .page("Accessibility")
        .field_type(FieldType::Slider {
            min: 0.0,
            max: 1.0,
            step: 0.05,
        })
        .validator(Validator::float_range(0.0, 1.0)),
    )
    .setting(
        "auto_sprint",
        SchemaEntry::new(
            "Automatically sprint when moving forward (toggle instead of hold)",
            false,
        )
        .label("Auto Sprint")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "hold_to_crouch",
        SchemaEntry::new("Require holding the crouch key instead of toggling", true)
            .label("Hold to Crouch")
            .page("Accessibility")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "toggle_ads",
        SchemaEntry::new(
            "Toggle aim-down-sights instead of requiring the button to be held",
            false,
        )
        .label("Toggle ADS")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "narrate_ui",
        SchemaEntry::new("Read focused UI elements aloud using text-to-speech", false)
            .label("Narrate UI")
            .page("Accessibility")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "narration_speed",
        SchemaEntry::new("Text-to-speech narration speed multiplier", 1.0_f64)
            .label("Narration Speed")
            .page("Accessibility")
            .field_type(FieldType::Slider {
                min: 0.5,
                max: 3.0,
                step: 0.1,
            })
            .validator(Validator::float_range(0.5, 3.0)),
    )
    .setting(
        "large_cursor",
        SchemaEntry::new("Display a larger mouse cursor for better visibility", false)
            .label("Large Cursor")
            .page("Accessibility")
            .field_type(FieldType::Checkbox),
    )
    .setting(
        "ui_border_highlight",
        SchemaEntry::new(
            "Add high-contrast borders to interactive UI elements",
            false,
        )
        .label("UI Border Highlight")
        .page("Accessibility")
        .field_type(FieldType::Checkbox),
    )
    .setting(
        "safe_zone_margin",
        SchemaEntry::new(
            "Extra inset margin percentage for HUD elements on older displays",
            0.0_f64,
        )
        .label("Safe Zone Margin (%)")
        .page("Accessibility")
        .field_type(FieldType::Slider {
            min: 0.0,
            max: 15.0,
            step: 0.5,
        })
        .validator(Validator::float_range(0.0, 15.0)),
    )
    .setting(
        "gameplay_hints_verbosity",
        SchemaEntry::new(
            "How often the game displays contextual gameplay hints",
            "normal",
        )
        .label("Gameplay Hints")
        .page("Accessibility")
        .field_type(FieldType::Dropdown {
            options: vec![
                DropdownOption::new("None", "none"),
                DropdownOption::new("Minimal", "minimal"),
                DropdownOption::new("Normal", "normal"),
                DropdownOption::new("Verbose", "verbose"),
            ],
        })
        .validator(Validator::string_one_of([
            "none", "minimal", "normal", "verbose",
        ])),
    );

    let _ = cfg.register(NS, OWNER, schema);
}
