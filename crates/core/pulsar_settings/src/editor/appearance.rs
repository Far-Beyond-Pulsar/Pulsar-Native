//! The editor's appearance: theme mode, color-blind mode, font size, corner
//! radius and scrollbars. `ui_common::appearance` applies them to the theme,
//! at startup and whenever the Editor Settings window changes one.

use pulsar_config::{
    ConfigManager, DropdownOption, FieldType, NamespaceSchema, SchemaEntry, Validator,
};

pub const NS: &str = "editor";
pub const OWNER: &str = "appearance";

pub const THEME_MODE: &str = "theme_mode";
pub const COLOR_BLIND_MODE: &str = "color_blind_mode";
pub const FONT_SIZE: &str = "font_size";
pub const RADIUS: &str = "radius";
pub const SCROLLBAR_SHOW: &str = "scrollbar_show";

pub fn register(cfg: &'static ConfigManager) {
    let schema = NamespaceSchema::new("Appearance", "Editor-wide visual preferences")
        .setting(
            THEME_MODE,
            SchemaEntry::new("Light or dark editor colors", "dark")
                .label("Theme")
                .page("Theme")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("Dark", "dark"),
                        DropdownOption::new("Light", "light"),
                    ],
                })
                .validator(Validator::string_one_of(["dark", "light"])),
        )
        .setting(
            COLOR_BLIND_MODE,
            SchemaEntry::new(
                "Adjust the editor's colors for a color vision deficiency",
                "none",
            )
            .label("Color-Blind Mode")
            .page("Theme")
            .field_type(FieldType::Dropdown {
                options: vec![
                    DropdownOption::new("None", "none"),
                    DropdownOption::new("Protanopia (red-blind)", "protanopia"),
                    DropdownOption::new("Deuteranopia (green-blind)", "deuteranopia"),
                    DropdownOption::new("Tritanopia (blue-blind)", "tritanopia"),
                    DropdownOption::new("Achromatopsia (monochrome)", "achromatopsia"),
                ],
            })
            .validator(Validator::string_one_of([
                "none",
                "protanopia",
                "deuteranopia",
                "tritanopia",
                "achromatopsia",
            ])),
        )
        .setting(
            FONT_SIZE,
            SchemaEntry::new("Base UI font size", 14_i64)
                .label("Font Size")
                .page("Layout")
                .field_type(FieldType::NumberInput {
                    min: Some(10.0),
                    max: Some(24.0),
                    step: Some(1.0),
                })
                .validator(Validator::int_range(10, 24)),
        )
        .setting(
            RADIUS,
            SchemaEntry::new("Corner radius", 6_i64)
                .label("Corner Radius")
                .page("Layout")
                .field_type(FieldType::Dropdown {
                    options: [0, 4, 6, 8]
                        .into_iter()
                        .map(|v| DropdownOption::new(format!("{v}px"), v.to_string()))
                        .collect(),
                }),
        )
        .setting(
            SCROLLBAR_SHOW,
            SchemaEntry::new("When scrollbars are visible", "scrolling")
                .label("Scrollbars")
                .page("Layout")
                .field_type(FieldType::Dropdown {
                    options: vec![
                        DropdownOption::new("While scrolling", "scrolling"),
                        DropdownOption::new("On hover", "hover"),
                        DropdownOption::new("Always", "always"),
                    ],
                }),
        );
    let _ = cfg.register(NS, OWNER, schema);
}
