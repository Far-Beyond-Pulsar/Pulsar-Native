//! Build Domain — build configuration, target platform, and the running game
//! process handle.
//!
//! This domain is consumed primarily by the toolbar's build buttons and the
//! game process lifecycle (spawn / kill / status polling).

use std::path::PathBuf;
use std::sync::Arc;

use super::editor::{BuildConfig, BuildMode, TargetPlatform};

/// Build and deployment domain.
#[derive(Clone)]
pub struct BuildDomain {
    /// Build profile (Debug / Release / Shipping).
    pub config: BuildConfig,
    /// Target platform for compilation.
    pub target_platform: TargetPlatform,
    /// Build action mode (Build / BuildAndRun / Check / etc.).
    pub mode: BuildMode,
    /// Whether the game process is currently running.
    pub game_running: bool,
    /// Handle to the running game child process.
    pub game_process: Arc<parking_lot::Mutex<Option<std::process::Child>>>,
    /// When set, the viewport should capture its framebuffer to this path on
    /// the next render frame.
    pub pending_thumbnail_capture: Option<PathBuf>,
}

impl Default for BuildDomain {
    fn default() -> Self {
        Self {
            config: BuildConfig::Debug,
            target_platform: TargetPlatform::WindowsX86_64Msvc,
            mode: BuildMode::Build,
            game_running: false,
            game_process: Arc::new(parking_lot::Mutex::new(None)),
            pending_thumbnail_capture: None,
        }
    }
}
