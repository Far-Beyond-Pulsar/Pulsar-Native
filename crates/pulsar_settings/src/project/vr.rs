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
                .field_type(FieldType::Checkbox))
        .setting("eye_resolution_scale",
            SchemaEntry::new("Per-eye resolution scale relative to the headset's recommended resolution", 1.0_f64)
                .label("Eye Resolution Scale").page("VR / XR")
                .field_type(FieldType::Slider { min: 0.5, max: 2.0, step: 0.05 })
                .validator(Validator::float_range(0.5, 2.0)))
        .setting("ipd_mm",
            SchemaEntry::new("Interpupillary distance override in millimeters (0 = use headset value)", 0.0_f64)
                .label("IPD (mm)").page("VR / XR")
                .field_type(FieldType::NumberInput { min: Some(0.0), max: Some(80.0), step: Some(0.5) })
                .validator(Validator::float_range(0.0, 80.0)))
        .setting("multiview_rendering",
            SchemaEntry::new("Render both eyes in a single pass using GPU multiview (requires hardware support)", false)
                .label("Multiview Rendering").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("stage_space",
            SchemaEntry::new("Reference space used for the play area", "local")
                .label("Stage Space").page("VR / XR")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Local (seated)", "local"),
                    DropdownOption::new("Stage (room-scale)", "stage"),
                    DropdownOption::new("View (head-relative)", "view"),
                ]})
                .validator(Validator::string_one_of(["local", "stage", "view"])))
        .setting("floor_level_correction",
            SchemaEntry::new("Apply an automatic floor level offset to align the virtual floor with the physical floor", true)
                .label("Floor Level Correction").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("height_offset_m",
            SchemaEntry::new("Manual vertical offset applied on top of automatic floor correction (meters)", 0.0_f64)
                .label("Height Offset (m)").page("VR / XR")
                .field_type(FieldType::NumberInput { min: Some(-2.0), max: Some(2.0), step: Some(0.01) })
                .validator(Validator::float_range(-2.0, 2.0)))
        .setting("haptics_enabled",
            SchemaEntry::new("Enable haptic feedback through XR controllers", true)
                .label("Haptics").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("haptics_intensity",
            SchemaEntry::new("Global multiplier for haptic feedback amplitude", 1.0_f64)
                .label("Haptics Intensity").page("VR / XR")
                .field_type(FieldType::Slider { min: 0.0, max: 1.0, step: 0.05 })
                .validator(Validator::float_range(0.0, 1.0)))
        .setting("teleport_locomotion",
            SchemaEntry::new("Use teleport as the primary movement method instead of smooth locomotion", false)
                .label("Teleport Locomotion").page("VR / XR")
                .field_type(FieldType::Checkbox))
        .setting("smooth_turn_speed_deg",
            SchemaEntry::new("Degrees per second for smooth rotation (0 = snap turn)", 0.0_f64)
                .label("Smooth Turn Speed (\u{00b0}/s)").page("VR / XR")
                .field_type(FieldType::Slider { min: 0.0, max: 360.0, step: 10.0 })
                .validator(Validator::float_range(0.0, 360.0)))
        .setting("snap_turn_angle_deg",
            SchemaEntry::new("Angle per snap rotation step in degrees", 45.0_f64)
                .label("Snap Turn Angle (\u{00b0})").page("VR / XR")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("15\u{00b0}", "15"),
                    DropdownOption::new("22.5\u{00b0}", "22"),
                    DropdownOption::new("30\u{00b0}", "30"),
                    DropdownOption::new("45\u{00b0}", "45"),
                    DropdownOption::new("90\u{00b0}", "90"),
                ]}))
        .setting("fade_on_teleport",
            SchemaEntry::new("Briefly fade to black when teleporting to reduce disorientation", true)
                .label("Fade on Teleport").page("VR / XR")
                .field_type(FieldType::Checkbox));

    let _ = cfg.register(NS, OWNER, schema);
}
