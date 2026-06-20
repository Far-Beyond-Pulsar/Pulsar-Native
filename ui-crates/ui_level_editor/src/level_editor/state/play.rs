//! Play Domain — parameters that control scene playback: time scale, target
//! frame rate, and multiplayer mode.
//!
//! These values are consumed by the toolbar's playback controls and affect
//! the game loop timing when the scene is played.

use super::editor::MultiplayerMode;

/// Playback domain — runtime parameters for scene simulation.
#[derive(Clone)]
pub struct PlayDomain {
    /// Game time scale (1.0 = real-time, 0.5 = half-speed, 2.0 = double-speed).
    pub time_scale: f32,
    /// Target frame rate for the game loop (0 = uncapped).
    pub target_fps: u32,
    /// Multiplayer networking mode.
    pub multiplayer_mode: MultiplayerMode,
}

impl Default for PlayDomain {
    fn default() -> Self {
        Self {
            time_scale: 1.0,
            target_fps: 60,
            multiplayer_mode: MultiplayerMode::Offline,
        }
    }
}
