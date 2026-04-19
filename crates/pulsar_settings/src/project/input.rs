use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "input";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Input", "Player input and control settings")
        .setting("mouse_sensitivity",
            SchemaEntry::new("Global mouse sensitivity multiplier", 1.0_f64)
                .label("Mouse Sensitivity").page("Input")
                .field_type(FieldType::Slider { min: 0.1, max: 5.0, step: 0.05 })
                .validator(Validator::float_range(0.1, 5.0)))
        .setting("invert_mouse_y",
            SchemaEntry::new("Invert the vertical mouse axis for camera look", false)
                .label("Invert Y (Mouse)").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("mouse_acceleration",
            SchemaEntry::new("Enable OS-level mouse pointer acceleration curve", false)
                .label("Mouse Acceleration").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("raw_mouse_input",
            SchemaEntry::new("Use raw device mouse input, bypassing OS acceleration", true)
                .label("Raw Mouse Input").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("gamepad_enabled",
            SchemaEntry::new("Enable gamepad / controller input", true)
                .label("Gamepad Support").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("gamepad_deadzone",
            SchemaEntry::new("Analog stick dead zone radius (0.0–1.0)", 0.1_f64)
                .label("Gamepad Deadzone").page("Input")
                .field_type(FieldType::Slider { min: 0.0, max: 0.5, step: 0.01 })
                .validator(Validator::float_range(0.0, 0.5)))
        .setting("gamepad_sensitivity",
            SchemaEntry::new("Gamepad analog stick sensitivity multiplier", 1.0_f64)
                .label("Gamepad Sensitivity").page("Input")
                .field_type(FieldType::Slider { min: 0.1, max: 5.0, step: 0.05 })
                .validator(Validator::float_range(0.1, 5.0)))
        .setting("gamepad_vibration",
            SchemaEntry::new("Enable haptic / rumble feedback from the gamepad", true)
                .label("Vibration / Rumble").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("gamepad_vibration_intensity",
            SchemaEntry::new("Vibration intensity multiplier", 1.0_f64)
                .label("Vibration Intensity").page("Input")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.05 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("touch_enabled",
            SchemaEntry::new("Enable touch screen input handling", false)
                .label("Touch Input").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("key_repeat_delay_ms",
            SchemaEntry::new("Delay before key repeat starts (ms)", 500_i64)
                .label("Key Repeat Delay (ms)").page("Input")
                .field_type(FieldType::NumberInput { min: Some(50.0), max: Some(2000.0), step: Some(10.0) })
                .validator(Validator::int_range(50, 2000)))
        .setting("key_repeat_rate_ms",
            SchemaEntry::new("Interval between key repeat events (ms)", 30_i64)
                .label("Key Repeat Rate (ms)").page("Input")
                .field_type(FieldType::NumberInput { min: Some(10.0), max: Some(500.0), step: Some(5.0) })
                .validator(Validator::int_range(10, 500)));

    let _ = cfg.register(NS, OWNER, schema);
}
