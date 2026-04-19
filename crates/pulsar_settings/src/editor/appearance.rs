use pulsar_config::{ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator};

pub const NS: &str = "editor";
pub const OWNER: &str = "appearance";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Appearance", "Visual appearance and theme settings")
        // ── Theme ──────────────────────────────────────────────────────────
        .setting("theme",
            SchemaEntry::new("Active UI theme", "Default Dark")
                .label("Theme").page("Appearance")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::same("Default Dark"),
                    DropdownOption::same("Default Light"),
                    DropdownOption::same("Catppuccin"),
                    DropdownOption::same("Tokyo Night"),
                    DropdownOption::same("Gruvbox"),
                    DropdownOption::same("Solarized"),
                    DropdownOption::same("Everforest"),
                    DropdownOption::same("Ayu"),
                    DropdownOption::same("Nord"),
                    DropdownOption::same("Dracula"),
                    DropdownOption::same("One Dark"),
                ]}))
        .setting("ui_scale",
            SchemaEntry::new("Scale factor for all UI elements", 1.0_f64)
                .label("UI Scale").page("Appearance")
                .field_type(FieldType::Slider { min: 0.5, max: 3.0, step: 0.05 })
                .validator(Validator::float_range(0.5, 3.0)))
        .setting("accent_color",
            SchemaEntry::new("Primary accent color used throughout the interface", "#0ea5e9")
                .label("Accent Color").page("Appearance")
                .field_type(FieldType::ColorPicker))
        .setting("icon_theme",
            SchemaEntry::new("Icon set used in the editor", "default")
                .label("Icon Theme").page("Appearance")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::same("default"),
                    DropdownOption::same("minimal"),
                    DropdownOption::same("colorful"),
                ]}))
        // ── Fonts ──────────────────────────────────────────────────────────
        .setting("ui_font_family",
            SchemaEntry::new("Font family for all UI text", "System Default")
                .label("UI Font Family").page("Appearance")
                .field_type(FieldType::TextInput { placeholder: Some("System Default".into()), multiline: false }))
        .setting("ui_font_size",
            SchemaEntry::new("Base font size for all UI elements (pt)", 13_i64)
                .label("UI Font Size").page("Appearance")
                .field_type(FieldType::NumberInput { min: Some(8.0), max: Some(24.0), step: Some(1.0) })
                .validator(Validator::int_range(8, 24)))
        // ── Layout ─────────────────────────────────────────────────────────
        .setting("compact_mode",
            SchemaEntry::new("Reduce padding and spacing for a denser layout", false)
                .label("Compact Mode").page("Appearance")
                .field_type(FieldType::Checkbox))
        .setting("show_status_bar",
            SchemaEntry::new("Show the status bar at the bottom of the editor", true)
                .label("Show Status Bar").page("Appearance")
                .field_type(FieldType::Checkbox))
        .setting("show_activity_bar",
            SchemaEntry::new("Show the activity bar on the side", true)
                .label("Show Activity Bar").page("Appearance")
                .field_type(FieldType::Checkbox))
        .setting("sidebar_position",
            SchemaEntry::new("Which side the primary sidebar appears on", "left")
                .label("Sidebar Position").page("Appearance")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Left", "left"),
                    DropdownOption::new("Right", "right"),
                ]})
                .validator(Validator::string_one_of(["left", "right"])))
        .setting("tab_bar_style",
            SchemaEntry::new("Visual style of editor tab bars", "default")
                .label("Tab Bar Style").page("Appearance")
                .field_type(FieldType::Dropdown { options: vec![
                    DropdownOption::new("Default", "default"),
                    DropdownOption::new("Compact", "compact"),
                    DropdownOption::new("Pill", "pill"),
                ]}))
        // ── Animations ─────────────────────────────────────────────────────
        .setting("animations_enabled",
            SchemaEntry::new("Enable UI animations and transitions", true)
                .label("Enable Animations").page("Appearance")
                .field_type(FieldType::Checkbox))
        .setting("animation_speed",
            SchemaEntry::new("Speed multiplier for UI animations (1.0 = normal)", 1.0_f64)
                .label("Animation Speed").page("Appearance")
                .field_type(FieldType::Slider { min: 0.1, max: 3.0, step: 0.1 })
                .validator(Validator::float_range(0.1, 3.0)));

    let _ = cfg.register(NS, OWNER, schema);
}
