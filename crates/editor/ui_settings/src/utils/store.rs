//! Reading and writing one setting's value.
//!
//! Editor and project settings live straight in the settings database (saved
//! by `GlobalSettings` / `ProjectSettings`). Plugin-namespace settings are
//! layered over their defaults (`engine_state::plugin_settings`): a write is
//! a project override, and writing the default value removes the override.

use engine_state::plugin_settings;
use engine_state::{global_config, ConfigValue};

use super::model::SettingId;

/// The value in effect now (`None`: the owner is not registered).
pub fn read(id: &SettingId) -> Option<ConfigValue> {
    global_config().get(&id.namespace, &id.owner, &id.key).ok()
}

/// The schema default.
pub fn default_of(id: &SettingId) -> Option<ConfigValue> {
    global_config()
        .owner_handle(&id.namespace, &id.owner)?
        .list_settings()
        .into_iter()
        .find(|info| info.key == id.key)
        .map(|info| info.default_value)
}

/// Write `value` (validated by the schema).
pub fn write(id: &SettingId, value: ConfigValue) -> Result<(), String> {
    if id.is_plugin() {
        if default_of(id).as_ref() == Some(&value) {
            plugin_settings::reset(&id.owner, &id.key);
            return Ok(());
        }
        return plugin_settings::set(&id.owner, &id.key, value)
            .map_err(|error| format!("{error:#}"));
    }
    global_config()
        .owner_handle(&id.namespace, &id.owner)
        .ok_or_else(|| {
            format!(
                "settings `{}/{}` are not registered",
                id.namespace, id.owner
            )
        })?
        .set(&id.key, value)
        .map_err(|error| error.to_string())
}

/// Put the setting back to its default.
pub fn reset(id: &SettingId) -> Result<(), String> {
    if id.is_plugin() {
        plugin_settings::reset(&id.owner, &id.key);
        return Ok(());
    }
    global_config()
        .owner_handle(&id.namespace, &id.owner)
        .ok_or_else(|| {
            format!(
                "settings `{}/{}` are not registered",
                id.namespace, id.owner
            )
        })?
        .reset_to_default(&id.key)
        .map_err(|error| error.to_string())
}

/// Whether the value differs from the default.
pub fn is_changed(id: &SettingId) -> bool {
    match (read(id), default_of(id)) {
        (Some(value), Some(default)) => value != default,
        _ => false,
    }
}

/// `value` for a number field: an integer setting stays an integer (a
/// fractional or out-of-range entry is refused), a float stays a float.
pub fn number_value(value: f64, like: &ConfigValue) -> Result<ConfigValue, &'static str> {
    match like {
        ConfigValue::Int(_) => {
            if !value.is_finite() || value.fract() != 0.0 {
                return Err("integer settings require a finite whole number");
            }
            if value < i64::MIN as f64 || value >= i64::MAX as f64 {
                return Err("integer setting value is outside the i64 range");
            }
            Ok(ConfigValue::Int(value as i64))
        }
        ConfigValue::Float(_) => Ok(ConfigValue::Float(value)),
        _ => Err("number setting has a non-numeric value"),
    }
}

#[cfg(test)]
mod tests {
    use super::number_value;
    use engine_state::ConfigValue;

    #[test]
    fn integers_stay_integers_and_floats_floats() {
        assert_eq!(
            number_value(12.0, &ConfigValue::Int(1)),
            Ok(ConfigValue::Int(12))
        );
        assert_eq!(
            number_value(12.5, &ConfigValue::Float(1.0)),
            Ok(ConfigValue::Float(12.5))
        );
        assert!(number_value(12.5, &ConfigValue::Int(1)).is_err());
        assert!(number_value(f64::NAN, &ConfigValue::Int(1)).is_err());
    }
}
