//! `StatusBarView` — the level editor status bar as its own cached GPUI view.
//!
//! Same motivation as [`ToolbarView`](super::toolbar::ToolbarView): rendered
//! inline it rebuilt five formatted i18n strings (including a scene-database
//! lookup for the selected object's name) on every dirty frame of
//! `LevelEditorPanel`, which is every frame Helio publishes.
//!
//! As a cached entity it re-renders only when its [`StatusBarSignature`]
//! changes. `scene.revision` stands in for the object count and the selected
//! object's name — both are derived from the scene database, and every mutation
//! path bumps the revision.

use std::sync::Arc;

use gpui::*;
use rust_i18n::t;
use ui::dock::PanelEvent;
use ui_common::StatusBar;

use crate::level_editor::scene_database::ObjectId;
use crate::level_editor::ui::frame_pump::spawn_frame_pump;
use crate::level_editor::{CameraMode, LevelEditorState, TransformTool};

/// Everything the status bar's text depends on.
///
/// **If you add a field to the status bar, add it here too**, or the bar will
/// render stale.
#[derive(Clone, PartialEq)]
struct StatusBarSignature {
    /// Covers the object count and the selected object's name: both are read
    /// out of the scene database, and every mutation bumps the revision.
    scene_revision: u64,
    selected: Option<ObjectId>,
    show_grid: bool,
    camera_mode: CameraMode,
    current_tool: TransformTool,
}

impl StatusBarSignature {
    fn of(state: &LevelEditorState) -> Self {
        Self {
            scene_revision: state.scene.revision,
            selected: state.scene.selected_object(),
            show_grid: state.editor.show_grid,
            camera_mode: state.editor.camera_mode,
            current_tool: state.editor.current_tool,
        }
    }
}

pub struct StatusBarView {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    last_signature: StatusBarSignature,
    pump_started: bool,
}

impl StatusBarView {
    pub fn new(state: Arc<parking_lot::RwLock<LevelEditorState>>) -> Self {
        let last_signature = StatusBarSignature::of(&state.read());
        Self {
            state,
            last_signature,
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
        self.start_pump(window, cx);

        let state = self.state.read();
        self.last_signature = StatusBarSignature::of(&state);

        let objects_count = state.scene.scene_objects().len();
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
        drop(state);

        StatusBar::new()
            .add_left_item(t!("LevelEditor.StatusBar.Objects", count => objects_count).to_string())
            .add_left_item(t!("LevelEditor.StatusBar.Selected", name => &selected_name).to_string())
            .add_right_item(camera_mode_str)
            .add_right_item(grid_status)
            .add_right_item(t!("LevelEditor.StatusBar.Tool", name => &tool_name).to_string())
            .render(cx)
    }
}
