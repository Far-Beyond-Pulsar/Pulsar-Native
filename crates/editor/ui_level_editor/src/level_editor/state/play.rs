//! Play Domain — parameters that control scene playback: time scale, target
//! frame rate, and multiplayer mode.
//!
//! These values are consumed by the toolbar's playback controls and affect
//! the game loop timing when the scene is played.

use std::path::PathBuf;

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
    /// Play-In-Editor control channel between the toolbar handlers (which start
    /// the build) and the viewport (which owns the non-`Send` `PieHost`).
    pub pie: PieControl,
}

impl Default for PlayDomain {
    fn default() -> Self {
        Self {
            time_scale: 1.0,
            target_fps: 60,
            multiplayer_mode: MultiplayerMode::Offline,
            pie: PieControl::default(),
        }
    }
}

/// Cross-thread control for Play-In-Editor (issue #243).
///
/// The `PieHost` itself lives on the viewport (main-thread, `!Send`); this
/// struct carries only `Send + Sync` signals through the shared state:
/// * toolbar `Play` → background `cargo build --lib` → sets [`Self::pending_start`]
/// * viewport sees `pending_start`, loads the dylib, sets [`Self::active`]
/// * toolbar `Stop` → sets [`Self::stop_requested`]; viewport drops the host
#[derive(Clone, Default)]
pub struct PieControl {
    /// A finished build waiting for the viewport to load. Consumed (taken) by the
    /// viewport on the next frame.
    pub pending_start: Option<PieStartRequest>,
    /// True once the viewport has a live embedded game.
    pub active: bool,
    /// True while the background build runs (nothing to show yet).
    pub building: bool,
    /// Set on Stop; the viewport tears down the host and clears it.
    pub stop_requested: bool,
    /// Last build/start error, surfaced by the viewport/toolbar.
    pub last_error: Option<String>,
}

/// Everything the viewport needs to load an embedded game.
#[derive(Clone)]
pub struct PieStartRequest {
    /// Path to the freshly built game `cdylib`.
    pub dylib_path: PathBuf,
    /// Game project root (contains `Cargo.toml` and `.pulsar/`).
    pub project_root: PathBuf,
    /// Path to the `.level` written from the editor's current `SceneDb`.
    pub scene_path: PathBuf,
}
