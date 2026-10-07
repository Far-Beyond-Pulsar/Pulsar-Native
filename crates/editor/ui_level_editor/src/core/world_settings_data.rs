//! Persisted per-level world settings.
//!
//! The settings are intentionally kept small and independent of renderer
//! configuration. They describe how a level should start when run.

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
        }
    }
}
