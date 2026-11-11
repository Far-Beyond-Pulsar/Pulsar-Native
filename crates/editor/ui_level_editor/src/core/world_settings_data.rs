//! Persisted per-level world settings.
//!
//! The settings are intentionally kept small and independent of renderer
//! configuration. They describe how a level should start when run. The
//! shell's own are the simulation clock and the render pipeline; everything
//! else lives in [`WorldSettingsData::values`] under the key its owner
//! registered: each role's implementer (`pulsar_traits::RoleRegistration`'s
//! `setting_key`) and each plugin's field
//! ([`crate::extend::WorldSettingField`]). Keys nobody registers now (a
//! disabled plugin's) are kept as they were, so the level loses nothing.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Gameplay and simulation defaults stored in each `.level` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldSettingsData {
    /// Multiplier applied to simulation time.
    pub time_scale: f32,
    /// Simulation step duration, in seconds.
    pub fixed_timestep: f32,
    /// Whether simulation advances automatically during play.
    pub auto_simulation: bool,
    /// The render pipeline the level runs: the id of a built-in Helio
    /// pipeline (`helio.fxaa`) or the project-relative path of a render-graph
    /// asset. `None`, or an id no longer registered, runs the engine default.
    pub render_pipeline: Option<String>,
    /// Every registered setting by key: role selections (project-relative
    /// paths) and plugins' fields. Absent keys are their defaults.
    #[serde(flatten)]
    pub values: Map<String, Value>,
}

impl Default for WorldSettingsData {
    fn default() -> Self {
        Self {
            time_scale: 1.0,
            fixed_timestep: 0.02,
            auto_simulation: true,
            render_pipeline: None,
            values: Map::new(),
        }
    }
}

impl WorldSettingsData {
    /// The value stored under `key`, if any.
    pub fn value(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    /// Store `value` under `key`; `None` removes it (its default applies).
    pub fn set_value(&mut self, key: &str, value: Option<Value>) {
        match value {
            Some(value) => {
                self.values.insert(key.to_owned(), value);
            }
            None => {
                self.values.remove(key);
            }
        }
    }

    /// The implementer selected for the role whose setting key is `key`.
    pub fn role_asset(&self, key: &str) -> Option<&str> {
        self.value(key)
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
    }

    /// Select `asset` for the role whose setting key is `key`; `None` or an
    /// empty path clears it.
    pub fn set_role_asset(&mut self, key: &str, asset: Option<String>) {
        self.set_value(
            key,
            asset
                .map(|path| path.trim().to_owned())
                .filter(|path| !path.is_empty())
                .map(Value::String),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_traits::builtin::{GAME_MODE_ROLE, GAME_STATE_ROLE};
    use pulsar_traits::roles::{roles, world_settings_of, RoleSelection};

    #[test]
    fn levels_saved_before_the_registry_still_load_and_keep_every_key() {
        let old = r#"{"some_vector":[0.0,-9.81,0.0],"some_flag":false,"time_scale":2.0,
            "game_mode_blueprint":"classes/Mode/graph_save.json","some_asset":"a/Game.asset"}"#;
        let settings: WorldSettingsData = serde_json::from_str(old).unwrap();
        assert_eq!(settings.time_scale, 2.0);
        assert_eq!(settings.fixed_timestep, 0.02);
        assert_eq!(
            settings.role_asset("game_mode_blueprint"),
            Some("classes/Mode/graph_save.json")
        );
        assert_eq!(settings.role_asset("player_controller_blueprint"), None);
        // Plugins' settings round-trip whether or not their plugin is
        // enabled.
        let saved = serde_json::to_value(&settings).unwrap();
        let original: Value = serde_json::from_str(old).unwrap();
        for key in ["some_vector", "some_flag", "some_asset"] {
            assert_eq!(saved[key], original[key], "{key}");
        }
    }

    #[test]
    fn every_role_round_trips_through_the_key_the_runtime_reads() {
        let mut settings = WorldSettingsData::default();
        for (i, role) in roles().into_iter().enumerate() {
            settings.set_role_asset(role.setting_key, Some(format!("classes/R{i}/graph_save.json")));
        }
        let level =
            serde_json::json!({ "world_settings": serde_json::to_value(&settings).unwrap() });
        let selection = RoleSelection::from_world_settings(&world_settings_of(&level.to_string()));
        for (i, role) in roles().into_iter().enumerate() {
            assert_eq!(
                selection.get(role.id),
                Some(format!("classes/R{i}/graph_save.json").as_str()),
                "{}",
                role.setting_key
            );
        }
        assert!(selection.get(GAME_MODE_ROLE).is_some());
        let state_key = pulsar_traits::roles::role(GAME_STATE_ROLE)
            .unwrap()
            .setting_key;
        settings.set_role_asset(state_key, Some("  ".into()));
        assert_eq!(settings.role_asset(state_key), None);
    }
}
