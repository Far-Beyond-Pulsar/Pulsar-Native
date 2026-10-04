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

pub use engine_state::playback::MultiplayerMode;

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

    // ── Tool Modes ────────────────────────────────────────────────────────
    pub tool_mode_registry: crate::tool_modes::ToolModeRegistry,
    /// Terrain sculpt and material-paint settings persist across mode switches.
    pub terrain: super::terrain::TerrainDomain,
    /// Spline tool selection, drawing plane and viewport display preferences.
    /// Authored curves live on scene objects and save with the level.
    pub spline: super::spline::SplineDomain,
    /// Voxel sculpt brush settings.
    pub voxel: super::voxel::VoxelSculptDomain,
    /// The sculpt stroke in progress, recorded as one undo step.
    pub voxel_stroke: Option<super::voxel::VoxelStroke>,
}

impl Default for EditorDomain {
    fn default() -> Self {
        // The built-in level mode is extended by registered modes.
        let mut tool_mode_registry = crate::tool_modes::ToolModeRegistry::builtin();
        crate::tool_modes::register_tool_modes(&mut tool_mode_registry);

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
            spline: super::spline::SplineDomain::default(),
            terrain: super::terrain::TerrainDomain::default(),
            voxel: super::voxel::VoxelSculptDomain::default(),
            voxel_stroke: None,
        }
    }
}

impl EditorDomain {
    /// Copy the editor inputs needed by read-only tool UI queries without
    /// copying the in-progress voxel stroke (which owns history snapshots).
    pub(crate) fn clone_for_tool_query(&self) -> Self {
        Self {
            current_tool: self.current_tool,
            camera_mode: self.camera_mode,
            camera_move_speed: self.camera_move_speed,
            show_wireframe: self.show_wireframe,
            show_lighting: self.show_lighting,
            show_grid: self.show_grid,
            feature_lighting_enabled: self.feature_lighting_enabled,
            feature_shadows_enabled: self.feature_shadows_enabled,
            feature_bloom_enabled: self.feature_bloom_enabled,
            feature_materials_enabled: self.feature_materials_enabled,
            tool_mode_registry: self.tool_mode_registry.clone(),
            terrain: self.terrain.clone(),
            spline: self.spline.clone(),
            voxel: self.voxel,
            voxel_stroke: None,
        }
    }

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
