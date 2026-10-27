//! The renderer's settings, from Helio's own description of them
//! (`helio_settings::GROUPS`): every setting Helio exposes appears here with
//! its label, page, range and default, under the group's owner (`rendering`,
//! `voxels`). The engine reads them back through the same description
//! (`engine_backend`'s `project_settings`).

use helio_settings::{Kind, Setting, Value};
use pulsar_config::{
    Color, ConfigManager, ConfigValue, DropdownOption, FieldType, NamespaceSchema, SchemaEntry,
    Validator,
};

pub const NS: &str = "project";
/// The renderer's owner (`helio_settings::RENDERER`).
pub const OWNER: &str = "rendering";

/// The config value of a Helio setting's value.
pub fn config_value(value: &Value) -> ConfigValue {
    match value {
        Value::Bool(b) => ConfigValue::Bool(*b),
        Value::Number(n) => ConfigValue::Float(*n),
        Value::Text(t) => ConfigValue::String(t.to_string()),
        Value::Color(c) => {
            let [r, g, b] = [c[0], c[1], c[2]].map(|x| (linear_to_srgb(x) * 255.0).round() as u8);
            ConfigValue::Color(Color {
                r,
                g,
                b,
                a: (c[3].clamp(0.0, 1.0) * 255.0).round() as u8,
            })
        }
    }
}

/// The Helio value of a stored config value (`None`: a type it cannot be).
pub fn helio_value(value: ConfigValue) -> Option<Value> {
    Some(match value {
        ConfigValue::Bool(b) => Value::Bool(b),
        ConfigValue::Int(i) => Value::Number(i as f64),
        ConfigValue::Float(f) => Value::Number(f),
        ConfigValue::String(s) => Value::Text(s.into()),
        ConfigValue::Color(c) => Value::Color([
            srgb_to_linear(f32::from(c.r) / 255.0),
            srgb_to_linear(f32::from(c.g) / 255.0),
            srgb_to_linear(f32::from(c.b) / 255.0),
            f32::from(c.a) / 255.0,
        ]),
        ConfigValue::Array(_) => return None,
    })
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn entry(group: &helio_settings::Group, setting: &Setting) -> SchemaEntry {
    let entry = SchemaEntry::new(setting.description, config_value(&setting.default))
        .label(setting.label)
        .page(format!("{} / {}", group.title, setting.page));
    match setting.kind {
        Kind::Toggle => entry.field_type(FieldType::Checkbox),
        Kind::Number { min, max, step } => entry
            .field_type(FieldType::Slider { min, max, step })
            .validator(Validator::float_range(min, max)),
        Kind::Integer { min, max } => entry.field_type(FieldType::NumberInput {
            min: Some(min as f64),
            max: Some(max as f64),
            step: Some(1.0),
        }),
        Kind::Choice(options) => entry.field_type(FieldType::Dropdown {
            options: options
                .iter()
                .map(|o| DropdownOption::new(o.label, o.value))
                .collect(),
        }),
        Kind::Color => entry.field_type(FieldType::ColorPicker),
    }
}

pub fn register(cfg: &'static ConfigManager) {
    for group in helio_settings::GROUPS {
        let schema = group.settings.iter().fold(
            NamespaceSchema::new(group.title, group.description),
            |schema, setting| schema.setting(setting.key, entry(group, setting)),
        );
        let _ = cfg.register(NS, group.key, schema);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_round_trips_through_the_config() {
        for group in helio_settings::GROUPS {
            for setting in group.settings {
                let back =
                    helio_value(config_value(&setting.default)).and_then(|v| setting.accept(v));
                assert_eq!(
                    back.as_ref(),
                    Some(&setting.default),
                    "{}.{}",
                    group.key,
                    setting.key
                );
            }
        }
    }
}
