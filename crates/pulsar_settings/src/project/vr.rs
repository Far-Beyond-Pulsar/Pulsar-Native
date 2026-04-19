use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "project";
pub const OWNER: &str = "vr";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("VR / XR", "Virtual and augmented reality settings")
        .setting("enable_vr",
            SchemaEntry::new("Enable VR/XR mode — requires a connected headset", false)
                .label("Enable VR").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("xr_runtime",
            SchemaEntry::new("OpenXR runtime backend to use", "auto")
                .label("XR Runtime").page("VR / XR")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Auto (system default)", "auto"),
                    DropdownOption::new("OpenXR", "openxr"),
                    DropdownOption::new("SteamVR / OpenVR", "openvr"),
                    DropdownOption::new("Oculus SDK", "oculus"),
                ]}))
        .setting("refresh_rate",
            SchemaEntry::new("Preferred display refresh rate in Hz (depends on headset capability)", "90")
                .label("Refresh Rate (Hz)").page("VR / XR")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("60 Hz", "60"),
                    DropdownOption::new("72 Hz", "72"),
                    DropdownOption::new("90 Hz", "90"),
                    DropdownOption::new("120 Hz", "120"),
                    DropdownOption::new("144 Hz", "144"),
                ]}))
        .setting("render_scale",
            SchemaEntry::new("Supersampling multiplier for VR eye buffers (1.0 = native resolution)", 1.0_f64)
                .label("Render Scale").page("VR / XR")
                .field_type(FieldType::Slider { min: 0.5, max: 2.0, step: 0.05 })
                .validator(Validator::float_range(0.5, 2.0)))
        .setting("foveated_rendering",
            SchemaEntry::new("Enable fixed or eye-tracked foveated rendering to improve performance", "none")
                .label("Foveated Rendering").page("VR / XR")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Off", "none"),
                    DropdownOption::new("Fixed (low)", "fixed_low"),
                    DropdownOption::new("Fixed (high)", "fixed_high"),
                    DropdownOption::new("Eye-Tracked (ETR)", "eye_tracked"),
                ]}))
        .setting("comfort_vignette",
            SchemaEntry::new("Show a vignette border during fast locomotion to reduce motion sickness", true)
                .label("Comfort Vignette").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("comfort_vignette_intensity",
            SchemaEntry::new("Strength of the comfort vignette effect", 0.6_f64)
                .label("Vignette Intensity").page("VR / XR")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.05 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("reprojection",
            SchemaEntry::new("Enable asynchronous space warp / motion reprojection for low-FPS recovery", true)
                .label("Reprojection").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("hand_tracking",
            SchemaEntry::new("Enable hand tracking input (requires headset support)", false)
                .label("Hand Tracking").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("passthrough",
            SchemaEntry::new("Enable mixed reality passthrough camera (requires headset support)", false)
                .label("Passthrough / MR").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("guardian_visible_in_editor",
            SchemaEntry::new("Show the VR guardian boundary mesh in the editor viewport", false)
                .label("Show Guardian in Editor").page("VR / XR")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
