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

use engine_backend::subsystems::render::helio_renderer::pipelines;

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
    /// The render pipeline the level runs: an id a provider offers
    /// (`helio.deferred`) or the project-relative path of a render-graph asset.
    /// There is no default: `None`, or an id that is not registered, runs no
    /// pipeline and the viewport says so.
    pub render_pipeline: Option<String>,
    /// What `render_pipeline` means (`pipelines::PIPELINE_VERSION`):
    /// 0 is a level saved when `None` meant the engine default, and loading
    /// migrates it ([`WorldSettingsData::migrate_level`]).
    pub pipeline_version: u32,
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
            pipeline_version: pipelines::PIPELINE_VERSION,
            values: Map::new(),
        }
    }
}

impl WorldSettingsData {
    /// Migrates a level file's JSON in place before it is read: a level saved
    /// when pipelines were implicit (no `pipeline_version`) and named none ran
    /// the engine's default, so it is given that pipeline's id and the current
    /// version. A level written since keeps its `None`. A named pipeline that
    /// was renamed (Helio's old built-in ids, now the render pipelines
    /// plugin's assets) is rewritten to its new id
    /// (`pipelines::migrate_pipeline_id`).
    pub fn migrate_level(level: &mut Value) {
        let Some(root) = level.as_object_mut() else {
            return;
        };
        let settings = root
            .entry("world_settings")
            .or_insert_with(|| Value::Object(Map::new()));
        let Some(settings) = settings.as_object_mut() else {
            return;
        };
        let version = settings
            .get("pipeline_version")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32;
        let saved = settings
            .get("render_pipeline")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if version >= pipelines::PIPELINE_VERSION {
            if let Some(saved) = saved {
                settings.insert(
                    "render_pipeline".into(),
                    Value::String(pipelines::migrate_pipeline_id(saved)),
                );
            }
            return;
        }
        if let Some(migrated) = pipelines::migrated_render_pipeline(version, saved) {
            settings.insert("render_pipeline".into(), Value::String(migrated));
        }
        settings.insert(
            "pipeline_version".into(),
            Value::from(pipelines::PIPELINE_VERSION),
        );
    }

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

    #[test]
    fn levels_saved_with_an_implicit_pipeline_keep_rendering_and_new_ones_may_have_none() {
        let migrate = |text: &str| {
            let mut level: Value = serde_json::from_str(text).unwrap();
            WorldSettingsData::migrate_level(&mut level);
            serde_json::from_value::<WorldSettingsData>(level["world_settings"].clone()).unwrap()
        };
        for legacy in [
            r#"{"version":"2.1"}"#,
            r#"{"world_settings":{"time_scale":2.0}}"#,
            r#"{"world_settings":{"render_pipeline":null}}"#,
        ] {
            // The old default, as the enabled plugins name it now (the render
            // pipelines plugin's `Deferred.rendergraph` when it is linked).
            let settings = migrate(legacy);
            assert_eq!(
                settings.render_pipeline,
                Some(pipelines::migrate_pipeline_id(
                    pipelines::LEGACY_DEFAULT_PIPELINE.to_string()
                )),
                "{legacy}"
            );
            assert_eq!(settings.pipeline_version, pipelines::PIPELINE_VERSION);
        }
        // A saved choice is untouched, and so is an explicit None.
        assert_eq!(
            migrate(r#"{"world_settings":{"render_pipeline":"g/a.rendergraph"}}"#)
                .render_pipeline
                .as_deref(),
            Some("g/a.rendergraph")
        );
        assert_eq!(
            migrate(r#"{"world_settings":{"pipeline_version":1,"render_pipeline":null}}"#)
                .render_pipeline,
            None
        );
        // What the editor writes round-trips as no pipeline.
        let saved = serde_json::to_value(WorldSettingsData::default()).unwrap();
        let level = serde_json::json!({ "world_settings": saved });
        assert_eq!(migrate(&level.to_string()).render_pipeline, None);
    }
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
            settings.set_role_asset(
                role.setting_key,
                Some(format!("classes/R{i}/graph_save.json")),
            );
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
