//! Persisted per-level world settings.
//!
//! The settings are intentionally kept small and independent of renderer
//! configuration. They describe how a level should start when run.

use engine_fs::world_roles::EngineRole;
use serde::{Deserialize, Serialize};

/// Gameplay and simulation defaults stored in each `.level` file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldSettingsData {
    /// Acceleration applied to simulated bodies, in metres per second squared.
    pub gravity: [f32; 3],
    /// Multiplier applied to simulation time.
    pub time_scale: f32,
    /// Simulation step duration, in seconds.
    pub fixed_timestep: f32,
    /// Whether the level starts physics simulation.
    pub physics_enabled: bool,
    /// Whether simulation advances automatically during play.
    pub auto_simulation: bool,
    /// Project-relative path to the level's game mode Blueprint, when set.
    ///
    /// This is a path rather than a generated identifier so references remain
    /// stable across project rebuilds. The Blueprint index owns validation and
    /// resolution of this reference.
    pub game_mode_blueprint: Option<String>,
    /// Project-relative path to the Blueprint that controls each player
    /// (implements the `PlayerController` trait), when set. Like every role
    /// field, absent in levels saved before it existed.
    pub player_controller_blueprint: Option<String>,
    /// Project-relative path to the Blueprint spawned as each player's body
    /// (implements the `PlayerCharacter` trait), when set.
    pub player_character_blueprint: Option<String>,
    /// Project-relative path to the level's game state Blueprint (implements
    /// the `GameState` trait), when set.
    pub game_state_blueprint: Option<String>,
    /// Project-relative path of the `.inputmap` the player controller brings
    /// to its pawn when it possesses it (`engine_fs::world_roles::PLAYER_INPUT_MAPPING_KEY`),
    /// when set. Absent in levels saved before it existed.
    pub player_input_mapping: Option<String>,
    /// The render pipeline the level runs: the id of a built-in Helio
    /// pipeline (`helio.fxaa`) or the project-relative path of a render-graph
    /// asset. `None`, or an id no longer registered, runs the engine default.
    pub render_pipeline: Option<String>,
}

impl Default for WorldSettingsData {
    fn default() -> Self {
        Self {
            gravity: [0.0, -9.81, 0.0],
            time_scale: 1.0,
            fixed_timestep: 0.02,
            physics_enabled: true,
            auto_simulation: true,
            game_mode_blueprint: None,
            player_controller_blueprint: None,
            player_character_blueprint: None,
            game_state_blueprint: None,
            player_input_mapping: None,
            render_pipeline: None,
        }
    }
}

impl WorldSettingsData {
    /// The Blueprint selected for `role`. The field a role maps to is named
    /// by `engine_fs::world_roles::ROLES`' `setting_key`, which the runtime
    /// reads from the saved level.
    pub fn role_blueprint(&self, role: EngineRole) -> Option<&str> {
        match role {
            EngineRole::GameMode => &self.game_mode_blueprint,
            EngineRole::PlayerController => &self.player_controller_blueprint,
            EngineRole::PlayerCharacter => &self.player_character_blueprint,
            EngineRole::GameState => &self.game_state_blueprint,
        }
        .as_deref()
    }

    pub fn set_role_blueprint(&mut self, role: EngineRole, blueprint: Option<String>) {
        let slot = match role {
            EngineRole::GameMode => &mut self.game_mode_blueprint,
            EngineRole::PlayerController => &mut self.player_controller_blueprint,
            EngineRole::PlayerCharacter => &mut self.player_character_blueprint,
            EngineRole::GameState => &mut self.game_state_blueprint,
        };
        *slot = blueprint.filter(|path| !path.is_empty());
    }

    /// Choose the `.inputmap` the player controller possesses its pawn with;
    /// an empty path clears it.
    pub fn set_player_input_mapping(&mut self, mapping: Option<String>) {
        self.player_input_mapping = mapping.filter(|path| !path.trim().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_fs::world_roles::{RoleSelection, ROLES};

    #[test]
    fn levels_saved_before_the_new_roles_still_load() {
        let old = r#"{"gravity":[0.0,-9.81,0.0],"game_mode_blueprint":"classes/Mode/graph_save.json"}"#;
        let settings: WorldSettingsData = serde_json::from_str(old).unwrap();
        assert_eq!(
            settings.role_blueprint(EngineRole::GameMode),
            Some("classes/Mode/graph_save.json")
        );
        assert_eq!(settings.role_blueprint(EngineRole::PlayerController), None);
        assert_eq!(settings.time_scale, 1.0);
    }

    #[test]
    fn the_input_mapping_loads_from_old_levels_and_round_trips_through_the_runtime_key() {
        // A level from before the setting existed has none.
        let old: WorldSettingsData = serde_json::from_str(r#"{"time_scale":2.0}"#).unwrap();
        assert_eq!(old.player_input_mapping, None);
        assert_eq!(old.time_scale, 2.0);

        let mut settings = WorldSettingsData::default();
        settings.set_player_input_mapping(Some("input/Game.inputmap".into()));
        let level = serde_json::json!({ "world_settings": serde_json::to_value(&settings).unwrap() });
        assert_eq!(
            RoleSelection::from_level_json(&level.to_string()).input_mapping(),
            Some("input/Game.inputmap"),
            "{}",
            engine_fs::world_roles::PLAYER_INPUT_MAPPING_KEY
        );
        settings.set_player_input_mapping(Some("  ".into()));
        assert_eq!(settings.player_input_mapping, None);
    }

    #[test]
    fn every_role_round_trips_through_the_key_the_runtime_reads() {
        let mut settings = WorldSettingsData::default();
        for (i, role) in EngineRole::ALL.into_iter().enumerate() {
            settings.set_role_blueprint(role, Some(format!("classes/R{i}/graph_save.json")));
        }
        let level = serde_json::json!({ "world_settings": serde_json::to_value(&settings).unwrap() });
        let selection = RoleSelection::from_level_json(&level.to_string());
        for (i, spec) in ROLES.iter().enumerate() {
            assert_eq!(
                selection.get(spec.role),
                Some(format!("classes/R{i}/graph_save.json").as_str()),
                "{}",
                spec.setting_key
            );
        }
        settings.set_role_blueprint(EngineRole::GameState, Some(String::new()));
        assert_eq!(settings.role_blueprint(EngineRole::GameState), None);
    }
}
