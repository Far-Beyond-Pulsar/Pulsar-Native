//! Editor Domain — tool selection, camera preferences, rendering toggles
//!
//! This domain stores the editor's "configuration" preferences — settings that
//! a user would expect to persist across sessions (current tool, camera mode,
//! grid visibility, feature toggles, etc.).
//!
//! These fields are **not** directly related to scene content; they control how
//! the editor itself behaves and renders.

use serde::{Deserialize, Serialize};

// ── Transform Tool ─────────────────────────────────────────────────────────

/// Active transform gizmo mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformTool {
    Select,
    Move,
    Rotate,
    Scale,
}

// ── Camera Mode ───────────────────────────────────────────────────────────

/// Viewport camera projection / orientation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CameraMode {
    Perspective,
    Orthographic,
    Top,
    Front,
    Side,
}

// ── Multiplayer Mode ──────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MultiplayerMode {
    Offline,
    Host,
    Client,
}

// ── Build Configuration ───────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildConfig {
    Debug,
    Release,
    Shipping,
}

/// Which action the Build button's primary click performs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BuildMode {
    #[default]
    Build,
    BuildAndRun,
    Check,
    Update,
    UpdateBuildAndRun,
    BuildScratch,
    BuildAndRunScratch,
    CheckScratch,
}

/// Complete Rust target platform and architecture support (excluding WASM).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetPlatform {
    WindowsX86_64Msvc,
    WindowsI686Msvc,
    WindowsAarch64Msvc,
    WindowsX86_64Gnu,
    WindowsI686Gnu,
    LinuxX86_64Gnu,
    LinuxI686Gnu,
    LinuxAarch64Gnu,
    LinuxArmv7Gnueabihf,
    LinuxArmGnueabi,
    LinuxArmGnueabihf,
    LinuxMips64Gnuabi64,
    LinuxMips64elGnuabi64,
    LinuxMipsGnu,
    LinuxMipselGnu,
    LinuxPowerpc64Gnu,
    LinuxPowerpc64leGnu,
    LinuxPowerpcGnu,
    LinuxRiscv64Gc,
    LinuxS390xGnu,
    LinuxSparcv9,
    LinuxX86_64Musl,
    LinuxAarch64Musl,
    LinuxArmv7Musleabihf,
    LinuxMipselMusl,
    LinuxMipsMusl,
    MacOsX86_64,
    MacOsAarch64,
    IosAarch64,
    IosX86_64,
    IosAarch64Sim,
    AndroidAarch64,
    AndroidArmv7,
    AndroidI686,
    AndroidX86_64,
    FreeBsdX86_64,
    FreeBsdI686,
    NetBsdX86_64,
    OpenBsdX86_64,
    DragonFlyX86_64,
    SolarisSparcv9,
    SolarisX86_64,
    IlumosX86_64,
    RedoxX86_64,
    FuchsiaAarch64,
    FuchsiaX86_64,
    PlayStationPs4,
    PlayStationPs5,
    XboxOne,
    XboxSeriesXS,
    NintendoSwitch,
}

// ── Editor domain ─────────────────────────────────────────────────────────

/// Editor configuration — tool, camera, rendering preferences.
///
/// These fields control **how** the editor behaves, not **what** the scene contains.
#[derive(Clone)]
pub struct EditorDomain {
    /// Current transform tool (Select, Move, Rotate, Scale).
    pub current_tool: TransformTool,
    /// Viewport camera projection / orientation.
    pub camera_mode: CameraMode,
    /// Camera movement speed (shared between UI and input thread).
    pub camera_move_speed: f32,

    // ── Viewport rendering toggles ────────────────────────────────────────
    pub show_wireframe: bool,
    pub show_lighting: bool,
    pub show_grid: bool,

    // ── Helio feature toggles ─────────────────────────────────────────────
    pub feature_lighting_enabled: bool,
    pub feature_shadows_enabled: bool,
    pub feature_bloom_enabled: bool,
    pub feature_materials_enabled: bool,
}

impl Default for EditorDomain {
    fn default() -> Self {
        Self {
            current_tool: TransformTool::Move,
            camera_mode: CameraMode::Perspective,
            camera_move_speed: 10.0,
            show_wireframe: false,
            show_lighting: true,
            show_grid: true,
            feature_lighting_enabled: true,
            feature_shadows_enabled: true,
            feature_bloom_enabled: true,
            feature_materials_enabled: true,
        }
    }
}

impl EditorDomain {
    pub fn set_tool(&mut self, tool: TransformTool) {
        self.current_tool = tool;
    }

    pub fn set_camera_mode(&mut self, mode: CameraMode) {
        self.camera_mode = mode;
    }

    pub fn toggle_grid(&mut self) {
        self.show_grid = !self.show_grid;
    }

    pub fn toggle_wireframe(&mut self) {
        self.show_wireframe = !self.show_wireframe;
    }

    pub fn toggle_lighting(&mut self) {
        self.show_lighting = !self.show_lighting;
    }

    pub fn adjust_camera_move_speed(&mut self, delta: f32) {
        self.camera_move_speed = (self.camera_move_speed + delta).clamp(0.5, 100.0);
    }
}
