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
                .validator(Validator::int_range(10, 500)))
        .setting("gamepad_dead_zone",
            SchemaEntry::new("Radial dead zone for analog sticks (0.0 = none, 0.2 = typical)", 0.1_f64)
                .label("Gamepad Dead Zone").page("Input")
                .field_type(FieldType::Slider { min: 0.0, max: 0.5, step: 0.01 })
                .validator(Validator::float_range(0.0, 0.5)))
        .setting("gamepad_trigger_threshold",
            SchemaEntry::new("Analog trigger threshold below which the trigger is considered released", 0.1_f64)
                .label("Trigger Threshold").page("Input")
                .field_type(FieldType::Slider { min: 0.0, max: 0.5, step: 0.01 })
                .validator(Validator::float_range(0.0, 0.5)))
        .setting("invert_gamepad_y",
            SchemaEntry::new("Invert the gamepad right stick vertical axis", false)
                .label("Invert Gamepad Y").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("gyroscope_aim",
            SchemaEntry::new("Use controller gyroscope / accelerometer for additional aiming precision", false)
                .label("Gyroscope Aim").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("gyroscope_sensitivity",
            SchemaEntry::new("Sensitivity multiplier for gyroscope aiming", 1.0_f64)
                .label("Gyroscope Sensitivity").page("Input")
                .field_type(FieldType::Slider { min: 0.1, max: 5.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 5.0)))
        .setting("mouse_sensitivity_x",
            SchemaEntry::new("Horizontal mouse sensitivity multiplier", 1.0_f64)
                .label("Mouse Sensitivity X").page("Input")
                .field_type(FieldType::Slider { min: 0.1, max: 5.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 5.0)))
        .setting("mouse_sensitivity_y",
            SchemaEntry::new("Vertical mouse sensitivity multiplier", 1.0_f64)
                .label("Mouse Sensitivity Y").page("Input")
                .field_type(FieldType::Slider { min: 0.1, max: 5.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 5.0)))
        .setting("mouse_acceleration",
            SchemaEntry::new("Apply acceleration curve to mouse movement", false)
                .label("Mouse Acceleration").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("raw_mouse_input",
            SchemaEntry::new("Bypass OS cursor acceleration and use raw hardware mouse data", true)
                .label("Raw Mouse Input").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("pointer_smoothing",
            SchemaEntry::new("Number of frames over which pointer input is averaged for smoothing (1 = off)", 1_i64)
                .label("Pointer Smoothing Frames").page("Input")
                .field_type(FieldType::NumberInput { min: Some(1.0), max: Some(8.0), step: Some(1.0) })
                .validator(Validator::int_range(1, 8)))
        .setting("touch_enabled",
            SchemaEntry::new("Enable touch / stylus input", false)
                .label("Touch Input").page("Input")
                .field_type(FieldType::Checkbox))
        .setting("input_action_bindings_file",
            SchemaEntry::new("Path to the project's input action bindings JSON", "config/input_actions.json")
                .label("Action Bindings File").page("Input")
                .field_type(FieldType::TextInput { placeholder: Some("config/input_actions.json".into()), multiline: false }));

    let _ = cfg.register(NS, OWNER, schema);
}
