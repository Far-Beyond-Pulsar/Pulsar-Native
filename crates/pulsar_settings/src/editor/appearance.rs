use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "appearance";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Appearance", "Visual appearance and theme settings")
        // ── Theme ──────────────────────────────────────────────────────────
        .setting(
            "theme",
            SchemaEntry::new("Active UI theme", "Default Dark")
                .label("Theme")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
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
                    ],
                }),
        )
        .setting(
            "ui_scale",
            SchemaEntry::new("Scale factor for all UI elements", 1.0_f64)
                .label("UI Scale")
                .page("Appearance")
                .field_type(FieldType::Slider {
                    min: 0.5,
                    max: 3.0,
                    step: 0.05,
                })
                .validator(Validator::float_range(0.5, 3.0)),
        )
        .setting(
            "accent_color",
            SchemaEntry::new(
                "Primary accent color used throughout the interface",
                "#0ea5e9",
            )
            .label("Accent Color")
            .page("Appearance")
            .field_type(FieldType::ColorPicker),
        )
        .setting(
            "icon_theme",
            SchemaEntry::new("Icon set used in the editor", "default")
                .label("Icon Theme")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::same("default"),
                        DropdownOption::same("minimal"),
                        DropdownOption::same("colorful"),
                    ],
                }),
        )
        // ── Fonts ──────────────────────────────────────────────────────────
        .setting(
            "ui_font_family",
            SchemaEntry::new("Font family for all UI text", "System Default")
                .label("UI Font Family")
                .page("Appearance")
                .field_type(FieldType::TextInput {
                    placeholder: Some("System Default".into()),
                    multiline: false,
                }),
        )
        .setting(
            "ui_font_size",
            SchemaEntry::new("Base font size for all UI elements (pt)", 13_i64)
                .label("UI Font Size")
                .page("Appearance")
                .field_type(FieldType::NumberInput {
                    min: Some(8.0),
                    max: Some(24.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(8, 24)),
        )
        // ── Layout ─────────────────────────────────────────────────────────
        .setting(
            "compact_mode",
            SchemaEntry::new("Reduce padding and spacing for a denser layout", false)
                .label("Compact Mode")
                .page("Appearance")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "show_status_bar",
            SchemaEntry::new("Show the status bar at the bottom of the editor", true)
                .label("Show Status Bar")
                .page("Appearance")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "show_activity_bar",
            SchemaEntry::new("Show the activity bar on the side", true)
                .label("Show Activity Bar")
                .page("Appearance")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "sidebar_position",
            SchemaEntry::new("Which side the primary sidebar appears on", "left")
                .label("Sidebar Position")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Left", "left"),
                        DropdownOption::new("Right", "right"),
                    ],
                })
                .validator(Validator::string_one_of(["left", "right"])),
        )
        .setting(
            "tab_bar_style",
            SchemaEntry::new("Visual style of editor tab bars", "default")
                .label("Tab Bar Style")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Default", "default"),
                        DropdownOption::new("Compact", "compact"),
                        DropdownOption::new("Pill", "pill"),
                    ],
                }),
        )
        // ── Animations ─────────────────────────────────────────────────────
        .setting(
            "animations_enabled",
            SchemaEntry::new("Enable UI animations and transitions", true)
                .label("Enable Animations")
                .page("Appearance")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "animation_speed",
            SchemaEntry::new("Speed multiplier for UI animations (1.0 = normal)", 1.0_f64)
                .label("Animation Speed")
                .page("Appearance")
                .field_type(FieldType::Slider {
                    min: 0.1,
                    max: 3.0,
                    step: 0.1,
                })
                .validator(Validator::float_range(0.1, 3.0)),
        )
        .setting(
            "compact_mode",
            SchemaEntry::new(
                "Reduce padding and spacing throughout the UI for denser layouts",
                false,
            )
            .label("Compact Mode")
            .page("Appearance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "custom_title_bar",
            SchemaEntry::new(
                "Use the editor's custom title bar instead of the native OS one",
                true,
            )
            .label("Custom Title Bar")
            .page("Appearance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "window_opacity",
            SchemaEntry::new(
                "Editor window background opacity (1.0 = fully opaque)",
                1.0_f64,
            )
            .label("Window Opacity")
            .page("Appearance")
            .field_type(FieldType::Slider {
                min: 0.3,
                max: 1.0,
                step: 0.01,
            })
            .validator(Validator::float_range(0.3, 1.0)),
        )
        .setting(
            "panel_blur",
            SchemaEntry::new(
                "Apply frosted glass blur to transparent panel backgrounds",
                false,
            )
            .label("Panel Blur")
            .page("Appearance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "panel_blur_radius",
            SchemaEntry::new("Blur radius in pixels for frosted glass panels", 12.0_f64)
                .label("Blur Radius")
                .page("Appearance")
                .field_type(FieldType::Slider {
                    min: 1.0,
                    max: 40.0,
                    step: 1.0,
                })
                .validator(Validator::float_range(1.0, 40.0)),
        )
        .setting(
            "sidebar_width",
            SchemaEntry::new("Default width of the left sidebar panel in pixels", 260_i64)
                .label("Sidebar Width (px)")
                .page("Appearance")
                .field_type(FieldType::NumberInput {
                    min: Some(160.0),
                    max: Some(600.0),
                    step: Some(10.0),
                })
                .validator(Validator::int_range(160, 600)),
        )
        .setting(
            "bottom_panel_height",
            SchemaEntry::new(
                "Default height of the bottom panel (terminal / log / problems) in pixels",
                200_i64,
            )
            .label("Bottom Panel Height (px)")
            .page("Appearance")
            .field_type(FieldType::NumberInput {
                min: Some(80.0),
                max: Some(800.0),
                step: Some(10.0),
            })
            .validator(Validator::int_range(80, 800)),
        )
        .setting(
            "breadcrumbs",
            SchemaEntry::new(
                "Show breadcrumb navigation bar below the editor tab strip",
                true,
            )
            .label("Breadcrumbs")
            .page("Appearance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "show_line_numbers",
            SchemaEntry::new("Display line numbers in the code editor gutter", true)
                .label("Line Numbers")
                .page("Appearance")
                .field_type(FieldType::Checkbox),
        )
        .setting(
            "minimap",
            SchemaEntry::new(
                "Show a minimap overview on the right side of the code editor",
                false,
            )
            .label("Minimap")
            .page("Appearance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "minimap_side",
            SchemaEntry::new("Which side of the editor the minimap appears on", "right")
                .label("Minimap Side")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Left", "left"),
                        DropdownOption::new("Right", "right"),
                    ],
                })
                .validator(Validator::string_one_of(["left", "right"])),
        )
        .setting(
            "status_bar_visible",
            SchemaEntry::new(
                "Show the status bar at the bottom of the editor window",
                true,
            )
            .label("Status Bar")
            .page("Appearance")
            .field_type(FieldType::Checkbox),
        )
        .setting(
            "tab_bar_position",
            SchemaEntry::new("Position of the file tab bar", "top")
                .label("Tab Bar Position")
                .page("Appearance")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Top", "top"),
                        DropdownOption::new("Bottom", "bottom"),
                    ],
                })
                .validator(Validator::string_one_of(["top", "bottom"])),
        );

    let _ = cfg.register(NS, OWNER, schema);
}
