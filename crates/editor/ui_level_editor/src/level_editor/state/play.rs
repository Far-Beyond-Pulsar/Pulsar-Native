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
    /// Asset updates (e.g. an edited class) waiting to be forwarded to the
    /// running game; the viewport delivers them on the render thread.
    pub pending_asset_updates: Vec<plugin_editor_api::AssetUpdated>,
    /// Set with [`Self::stop_requested`] while a game runs: the viewport
    /// restores the editor world (`SceneDomain::exit_play_mode`) right
    /// after the game shut down, so no game code runs against the restored
    /// world (#925). See `end_pie`.
    pub restore_after_stop: bool,
    /// When the pending stop was requested, for the fallback restore when
    /// no viewport processes it.
    pub stop_requested_at: Option<std::time::Instant>,
    /// Pause (`Some(true)`) or resume (`Some(false)`) the running game's
    /// simulation; the viewport applies it on the render thread.
    pub pause_request: Option<bool>,
    /// Frames to step while paused; the viewport applies them.
    pub step_request: u32,
    /// Whether the game's simulation is paused (mirrored by the viewport).
    pub paused: bool,
    /// Whether the running game supports pause / step (games built before
    /// the control entry point do not).
    pub supports_control: bool,
    /// Script problems (errors with class / function / node) the running
    /// game reported this session, oldest first (bounded).
    pub problems: Vec<pulsar_events::ScriptProblem>,
    /// The library the running game was loaded from, and its modification
    /// time then. Pressing Play again reloads only the script classes when
    /// neither the sources nor this library changed since (#833).
    pub loaded_artifact: Option<(PathBuf, std::time::SystemTime)>,
}

/// Problems kept in [`PieControl::problems`].
pub const MAX_PIE_PROBLEMS: usize = 200;

/// Everything the viewport needs to load an embedded game.
#[derive(Clone)]
pub struct PieStartRequest {
    /// Path to the freshly built game `cdylib`.
    pub dylib_path: PathBuf,
    /// Game project root (contains `Cargo.toml` and `.pulsar/`).
    pub project_root: PathBuf,
    /// Path to the `.level` written from the editor's current `SceneDb`.
    pub scene_path: PathBuf,
    /// True when this load replaces a STILL-RUNNING game (#653): the shared
    /// world keeps its entities/components, and the guest re-binds actor
    /// registrations instead of spawning duplicates (native hot reload).
    pub reload: bool,
    /// True when a game is running and nothing native changed since it was
    /// loaded (#833): the viewport keeps the running game and reloads every
    /// script class in it instead of swapping the library.
    pub scripts_only: bool,
}
