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

    // ── Tool Mode & Terrain Domains ───────────────────────────────────────
    pub tool_mode_registry: crate::level_editor::tool_modes::ToolModeRegistry,
    pub terrain: super::terrain::TerrainDomain,
    /// Per-stroke voxel terrain undo history. Separate from
    /// `SceneDomain`'s undo stack on purpose -- that one snapshots the scene
    /// database, which does not contain voxels at all (design doc §5.5).
    pub terrain_undo: super::terrain_undo::TerrainUndoDomain,
    /// Handle to the renderer's terrain seam, stored by `TerrainMode::
    /// on_mode_entered` so mode-owned panels (which only receive the shared
    /// state) can list and create terrain bodies. `None` until Terrain mode
    /// has been entered with a live renderer.
    pub terrain_api: Option<engine_backend::services::terrain_edit::TerrainEditApi>,
    /// State for the Milestone 5 extensibility-demo `SplineMode`. Lives here
    /// (rather than as a field on `SplineMode` itself) because the toolbar
    /// and status bar both need to read it without going through the mode
    /// instance -- see `state/spline.rs`'s doc comment.
    pub spline: super::spline::SplineDomain,
}

impl Default for EditorDomain {
    fn default() -> Self {
        // `builtin()` registers the two Milestone 1-4 modes (LevelEdit,
        // Terrain); `register_tool_modes` then adds every mode registered
        // after them -- currently just Milestone 5's `SplineMode` -- purely
        // through `ToolModeRegistry::register`, with `builtin()`'s own two
        // entries untouched (design doc §9 / issue #714).
        let mut tool_mode_registry = crate::level_editor::tool_modes::ToolModeRegistry::builtin();
        crate::level_editor::tool_modes::register_tool_modes(&mut tool_mode_registry);

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
            tool_mode_registry,
            terrain: super::terrain::TerrainDomain::default(),
            terrain_undo: super::terrain_undo::TerrainUndoDomain::default(),
            terrain_api: None,
            spline: super::spline::SplineDomain::default(),
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
