//! The engine roles of the loaded level (game mode, player controller,
//! player character, game state).
//!
//! A level names, in its world settings, the Blueprint that plays each role
//! (`engine_fs::world_roles`). The loop resolves those names when the level
//! is loaded and keeps the result for the spawn step. Resolving only reads:
//! nothing here creates an object.
//!
//! # Where spawning goes
//!
//! Read [`TickLoop::world_roles`] from the script phase, before the first
//! `reconcile` of the session starts the level's scripts:
//!
//! - game mode and game state are one-per-level: start them as global
//!   script instances in `ScriptDriver::start_globals`
//!   (`scripting/driver.rs`), resolving each binding's
//!   [`class_name`](engine_fs::world_roles::RoleBinding::class_name) through
//!   `resolve_class_ref` like a `global_scripts` entry. Call the Blueprint's
//!   `on_game_start` / `on_init` (the `GameMode` / `GameState` trait
//!   methods) when it starts;
//! - player controller and player character are per player: when
//!   `GameMode.on_player_joined` fires, spawn the character with the same
//!   command path scripts use for `world::spawn` (`ScriptDriver::apply_commands`),
//!   start the controller as an instance without an object, then call
//!   `PlayerController.on_possess` and `PlayerCharacter.on_possessed`.
//!
//! A binding is spawnable when [`usable`](engine_fs::world_roles::WorldRoles::usable)
//! returns it; anything else is reported by
//! [`diagnostics`](engine_fs::world_roles::WorldRoles::diagnostics).

use std::path::Path;

use engine_fs::world_roles::WorldRoles;

use crate::tick::TickLoop;

impl TickLoop {
    /// The Blueprints the level assigned to the engine roles; empty until
    /// [`load_world_roles`](Self::load_world_roles) ran.
    pub fn world_roles(&self) -> &WorldRoles {
        &self.world_roles
    }

    /// Replace the roles directly (tests, or a host that resolved them).
    pub fn set_world_roles(&mut self, roles: WorldRoles) {
        if let Some(driver) = &self.scripts {
            driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .set_world_roles(roles.clone());
        }
        self.world_roles = roles;
    }

    /// Read the roles the level file at `level_path` selects and resolve
    /// them against the project at `project_root`. An unreadable level has
    /// no roles; selections that cannot be used are logged.
    pub fn load_world_roles(&mut self, level_path: &Path, project_root: &Path) {
        let roles = match std::fs::read_to_string(level_path) {
            Ok(json) => WorldRoles::for_level(&json, project_root),
            Err(error) => {
                tracing::debug!(level = %level_path.display(), "No world roles: {error}");
                WorldRoles::default()
            }
        };
        for message in roles.diagnostics() {
            tracing::warn!("{message}");
        }
        self.set_world_roles(roles);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_fs::world_roles::{BindingStatus, EngineRole};

    #[test]
    fn a_loaded_level_selects_its_roles() {
        let dir = tempfile::tempdir().unwrap();
        let level = dir.path().join("main.level");
        std::fs::write(
            &level,
            r#"{"world_settings":{"game_mode_blueprint":"classes/Mode/graph_save.json"}}"#,
        )
        .unwrap();
        let mut game = TickLoop::new(pulsar_core::TickMode::default(), 1);
        assert!(game.world_roles().is_empty());
        game.load_world_roles(&level, dir.path());
        let binding = game.world_roles().get(EngineRole::GameMode).unwrap();
        assert_eq!(binding.class_name(), Some("Mode"));
        // No trait index exists for this project, so the selection is
        // passed on unchecked rather than rejected.
        assert_eq!(binding.status, BindingStatus::Unchecked);

        game.load_world_roles(&dir.path().join("missing.level"), dir.path());
        assert!(game.world_roles().is_empty());
    }
}
