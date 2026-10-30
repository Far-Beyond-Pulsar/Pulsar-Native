//! The engine roles of the loaded level (game mode, player controller,
//! player character, game state).
//!
//! A level names, in its world settings, the Blueprint that plays each role
//! (`engine_fs::world_roles`). The loop resolves those names when the level
//! is loaded and keeps the result for the session start. Resolving only
//! reads: nothing here creates an object.
//!
//! # Role entities
//!
//! When the session's first script phase runs, `ScriptDriver::start_roles`
//! (`scripting/driver.rs`) spawns each usable binding as an **entity of its
//! class**, through the same world-command path scripts use for
//! `world::spawn`. A role entity is a session object: it is never part of the
//! level's data, so the level format and the editor's save path know nothing
//! of it. The driver tracks the entities it owns explicitly and despawns them
//! when the session ends (`ScriptDriver::end_play_all`), which matters under
//! Play-in-Editor, where the session adopts the editor's world.
//!
//! Each is stamped with [`RoleEntity`] so a session that finds one left in a
//! world it adopts (a hot reload that did not stop the old session cleanly)
//! re-binds it rather than spawning a second.
//!
//! The roles' trait methods are called through the trait
//! (`pulsar.GameMode`, ...), see `scripting/traits.rs`.

use std::path::Path;

use engine_fs::world_roles::{EngineRole, WorldRoles};

use crate::tick::TickLoop;

/// Marks the entity a session spawned for an engine role. Never saved:
/// role entities are not level data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleEntity {
    pub role: EngineRole,
}

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
