//! `StatusBarView` — the level editor status bar as its own cached GPUI view.
//!
//! Same motivation as [`ToolbarView`](super::toolbar::ToolbarView): rendered
//! inline it rebuilt five formatted i18n strings (including a scene-database
//! lookup for the selected object's name) on every dirty frame of
//! `LevelEditorPanel`, which is every frame Helio publishes.
//!
//! As a cached entity it re-renders only when its [`StatusBarSignature`]
//! changes. `store_revision` stands in for the object count and the selected
//! object's name — both are derived from the scene database, whose shared
//! store bumps the counter on every mutation from any thread.

use std::sync::Arc;

use gpui::*;
use rust_i18n::t;
use ui::dock::PanelEvent;
use ui_common::StatusBar;

use crate::level_editor::scene_database::ObjectId;
use crate::level_editor::tool_modes::{CameraFrame, ToolModeContext, ToolModeId, ViewportFrame};
use crate::level_editor::ui::frame_pump::spawn_frame_pump;
use crate::level_editor::{CameraMode, LevelEditorState, TransformTool};
use engine_backend::services::gpu_renderer::GpuRenderer;

/// Everything the status bar's text depends on.
///
/// **If you add a field to the status bar, add it here too**, or the bar will
/// render stale.
#[derive(Clone, PartialEq)]
struct StatusBarSignature {
    /// Covers the object count and the selected object's name: both are read
    /// out of the scene database, whose shared store bumps this counter on
    /// every mutation from any thread.
    store_revision: u64,
    selected: Option<ObjectId>,
    show_grid: bool,
    camera_mode: CameraMode,
    current_tool: TransformTool,
    tool_mode: ToolModeId,
    terrain_radius_m: f32,
    terrain_strength: f32,
    // Spline mode's status text depends on both of these (Milestone 5).
    spline_point_count: usize,
    spline_length_m: f32,
}

impl StatusBarSignature {
    fn of(state: &LevelEditorState) -> Self {
        Self {
            store_revision: state.scene.database.store_revision(),
            selected: state.scene.selected_object(),
            show_grid: state.editor.show_grid,
            camera_mode: state.editor.camera_mode,
            current_tool: state.editor.current_tool,
            tool_mode: state.editor.tool_mode_registry.selected_id(),
            terrain_radius_m: state.editor.terrain.sculpt.radius_m,
            terrain_strength: state.editor.terrain.sculpt.strength,
            spline_point_count: state.editor.spline.points.len(),
            spline_length_m: state.editor.spline.total_length_m(),
        }
    }
}

pub struct StatusBarView {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    last_signature: StatusBarSignature,
    /// Root-object count keyed by the store revision it was counted at.
    cached_root_count: Option<(u64, usize)>,
    pump_started: bool,
}

impl StatusBarView {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    ) -> Self {
        let last_signature = StatusBarSignature::of(&state.read());
        Self {
            state,
            gpu_engine,
            last_signature,
            cached_root_count: None,
            pump_started: false,
        }
    }

    /// Must match the root style of `ui_common::StatusBar::render` (`w_full`,
    /// `h_8`): a cached view lays itself out from this refinement alone.
    pub fn cache_style() -> StyleRefinement {
        StyleRefinement::default().w_full().h_8()
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let signature = StatusBarSignature::of(&this.state.read());
            if signature != this.last_signature {
                this.last_signature = signature;
                cx.notify();
            }
        });
    }
}

impl EventEmitter<PanelEvent> for StatusBarView {}

impl Render for StatusBarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("status bar: render");
        let _t = gpui::render_stats::scope("status bar: render");

        self.start_pump(window, cx);

        let state = self.state.read();
        self.last_signature = StatusBarSignature::of(&state);

        let objects_count = match self.cached_root_count {
            Some((revision, count)) if revision == state.scene.database.store_revision() => count,
            _ => {
                let count = state.scene.database.root_count();
                self.cached_root_count = Some((state.scene.database.store_revision(), count));
                count
            }
        };
        let selected_name = state
            .scene
            .selected_object()
            .and_then(|id| state.scene.database.get_object(&id))
            .map(|obj| obj.name.clone())
            .unwrap_or_else(|| t!("LevelEditor.StatusBar.None").to_string());

        let grid_status = if state.editor.show_grid {
            t!("LevelEditor.StatusBar.GridOn").to_string()
        } else {
            t!("LevelEditor.StatusBar.GridOff").to_string()
        };

        let camera_mode_str = match state.editor.camera_mode {
            CameraMode::Perspective => t!("LevelEditor.CameraMode.Perspective").to_string(),
            CameraMode::Orthographic => t!("LevelEditor.CameraMode.Orthographic").to_string(),
            CameraMode::Top => t!("LevelEditor.CameraMode.Top").to_string(),
            CameraMode::Front => t!("LevelEditor.CameraMode.Front").to_string(),
            CameraMode::Side => t!("LevelEditor.CameraMode.Side").to_string(),
        };

        let tool_name = match state.editor.current_tool {
            TransformTool::Select => t!("LevelEditor.Tool.Select").to_string(),
            TransformTool::Move => t!("LevelEditor.Tool.Move").to_string(),
            TransformTool::Rotate => t!("LevelEditor.Tool.Rotate").to_string(),
            TransformTool::Scale => t!("LevelEditor.Tool.Scale").to_string(),
        };

        let mode_status = {
            let mut state_clone = state.clone();
            let ctx = ToolModeContext {
                state: &mut state_clone,
                gpu_engine: &self.gpu_engine,
                // Presentational only: `status` reads editor state, never the
                // terrain seam, so this path must not take `gpu_engine` to
                // fetch one.
                terrain: None,
                camera: CameraFrame::default(),
                viewport: ViewportFrame::default(),
            };
            state.editor.tool_mode_registry.selected().status(&ctx)
        };
        drop(state);

        let mut bar = StatusBar::new()
            .add_left_item(t!("LevelEditor.StatusBar.Objects", count => objects_count).to_string())
            .add_left_item(t!("LevelEditor.StatusBar.Selected", name => &selected_name).to_string());

        if let Some(status) = mode_status {
            bar = bar.add_left_item(status.text);
        }

        bar.add_right_item(camera_mode_str)
            .add_right_item(grid_status)
            .add_right_item(t!("LevelEditor.StatusBar.Tool", name => &tool_name).to_string())
            .render(cx)
    }
}
