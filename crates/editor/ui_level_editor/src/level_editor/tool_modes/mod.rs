//! Level Editor Tool Modes
//!
//! Provides modular editing modes (e.g. Level Edit, Terrain) via a pluggable
//! registry pattern. Each mode encapsulates its identity, presentational widgets,
//! brush cursor rendering, status bar readouts, and pointer event handling.

pub mod dispatcher;
pub mod level_edit;
pub mod registry;
pub mod terrain;

use std::sync::Mutex;

use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::services::terrain_edit::TerrainEditApi;

pub use dispatcher::*;
pub use level_edit::*;
pub use registry::*;
pub use terrain::*;

use crate::level_editor::state::LevelEditorState;

// ── Frames & Cursor ────────────────────────────────────────────────────────

/// Camera snapshot for mode raycasting and spatial math.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraFrame {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fov: f32,
}

impl Default for CameraFrame {
    fn default() -> Self {
        Self {
            position: [0.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            fov: 60.0,
        }
    }
}

/// Viewport dimensions in logical/screen pixels.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct ViewportFrame {
    pub width: f32,
    pub height: f32,
}

/// Visual brush cursor ring or disk for spatial editing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrushCursor {
    pub center: [f32; 3],
    pub radius: f32,
    pub color: [f32; 4],
}

// ── Pointer Events & Results ───────────────────────────────────────────────

/// Type of pointer action.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerKind {
    Down,
    Up,
    Drag,
    Hover,
    Scroll { delta_y: f32 },
}

/// Normalized viewport coordinates and input state passed to active tool mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolPointerEvent {
    pub kind: PointerKind,
    pub button: Option<gpui::MouseButton>,
    pub norm_x: f32,
    pub norm_y: f32,
    pub holding_mods: gpui::Modifiers,
}

/// Result returned from [`ToolMode::on_pointer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolPointerResult {
    /// Mode consumed the event; default editor interaction will not run.
    Consumed,
    /// Mode passed the event through; editor will perform default object pick/transform gizmo.
    PassThrough,
}

// ── Declarative Toolbar Widgets ────────────────────────────────────────────

/// Declarative toolbar widget that a mode requests the toolbar shell to render.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolWidget {
    Slider {
        id: &'static str,
        label_key: &'static str,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
    },
    Segmented {
        id: &'static str,
        options: Vec<(&'static str, &'static str)>,
        selected: &'static str,
    },
    Toggle {
        id: &'static str,
        label_key: &'static str,
        on: bool,
    },
    Divider,
}

// ── Status Bar Readout ─────────────────────────────────────────────────────

/// Mode-provided status text and tooltip for the editor status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusReadout {
    pub text: String,
    pub tooltip: Option<String>,
}

// ── Mode Context ───────────────────────────────────────────────────────────

/// Execution context provided to tool mode operations.
pub struct ToolModeContext<'a> {
    pub state: &'a mut LevelEditorState,
    pub gpu_engine: &'a Mutex<GpuRenderer>,
    /// Voxel terrain seam (design doc §5.4). `None` when the renderer has not
    /// initialized yet; modes must treat that as "terrain editing is
    /// unavailable" and pass the event through, not as an error.
    ///
    /// This is what keeps `TerrainMode` off raw `TerrainRuntimeHandle`s: it
    /// never sees the runtime, only this seam.
    pub terrain: Option<&'a TerrainEditApi>,
    pub camera: CameraFrame,
    pub viewport: ViewportFrame,
}

// ── ToolMode Trait ─────────────────────────────────────────────────────────

/// Contract implemented by all level editor tool modes.
pub trait ToolMode: Send + Sync {
    /// Unique identifier for this tool mode.
    fn id(&self) -> ToolModeId;

    /// Localization key for the mode's display label.
    fn label_key(&self) -> &'static str;

    /// Icon representing this mode in the toolbar and mode indicator.
    fn icon(&self) -> ui::IconName;

    /// Localization key for the mode's description tooltip.
    fn description_key(&self) -> &'static str;

    /// Called when switching into this mode.
    fn on_mode_entered(&mut self, ctx: &mut ToolModeContext);

    /// Called when switching out of this mode.
    fn on_mode_exited(&mut self, ctx: &mut ToolModeContext);

    /// Optional 3D brush cursor ring/mesh to display in the viewport.
    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        None
    }

    /// Declarative widgets to display in the editor toolbar when active.
    fn toolbar_controls(&self, _ctx: &ToolModeContext) -> Vec<ToolWidget> {
        Vec::new()
    }

    /// Status readout to surface on the status bar when active.
    fn status(&self, _ctx: &ToolModeContext) -> Option<StatusReadout> {
        None
    }

    /// Handle pointer events occurring within the viewport.
    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult;

    /// Helper for boxing clones in the trait registry.
    fn clone_box(&self) -> Box<dyn ToolMode>;
}
