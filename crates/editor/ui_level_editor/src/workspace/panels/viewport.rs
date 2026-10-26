//! Viewport dock panel.

use crate::state::{CameraMode, LevelEditorState};
use crate::ui::ViewportPanel;
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::dock::{Panel, PanelEvent, PanelState};

/// Viewport Panel Wrapper
///
/// The editor opens with one of these (the primary viewport). The split
/// buttons add more, each a panel of its own with its own renderer and camera
/// over the same scene (see `LevelEditorPanel::split_viewports`).
pub struct ViewportPanelWrapper {
    viewport_panel: ViewportPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    focus_handle: FocusHandle,
    pump_started: bool,
    /// `true` for a viewport added by a split (not the primary one). Its tab
    /// is named after its view and it is never saved with the layout.
    split_off: bool,
}

impl ViewportPanelWrapper {
    pub(crate) fn is_camera_navigating(&self) -> bool {
        self.viewport_panel.is_camera_navigating()
    }

    pub fn new(
        viewport_panel: ViewportPanel,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            viewport_panel,
            state,
            gpu_engine,
            focus_handle: cx.focus_handle(),
            pump_started: false,
            split_off: false,
        }
    }

    /// Mark this as a viewport added by a split.
    pub(crate) fn split_off(mut self) -> Self {
        self.split_off = true;
        self
    }
}

impl EventEmitter<PanelEvent> for ViewportPanelWrapper {}

ui_common::panel_boilerplate!(ViewportPanelWrapper);

impl Render for ViewportPanelWrapper {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("viewport panel: render");
        let _t = gpui::render_stats::scope("viewport panel: render");

        if !self.pump_started {
            self.pump_started = true;
            let mut last = self
                .viewport_panel
                .controls_signature(&self.state.read(), cx);
            crate::ui::frame_pump::spawn_frame_pump(
                &cx.entity(),
                window,
                move |this, window, cx| {
                    // Before the signature: a changed camera mode must reach the
                    // renderer and re-draw the controls on the same tick.
                    this.viewport_panel
                        .sync_view_settings(&this.state, window, cx);
                    let next = this
                        .viewport_panel
                        .controls_signature(&this.state.read(), cx);
                    if next != last {
                        last = next;
                        cx.notify();
                    }
                },
            );
        }

        let state = self.state.read();
        self.viewport_panel
            .render(&state, self.state.clone(), &self.gpu_engine, cx)
    }
}

impl Panel for ViewportPanelWrapper {
    fn panel_name(&self) -> &'static str {
        if self.split_off {
            "viewport_split"
        } else {
            "viewport"
        }
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let Some(mode) = self.viewport_panel.own_camera_mode() else {
            return "Viewport".into_any_element();
        };
        match mode {
            CameraMode::Perspective => t!("LevelEditor.Camera.PerspectiveView"),
            CameraMode::Orthographic => t!("LevelEditor.Camera.OrthographicView"),
            CameraMode::Top => t!("LevelEditor.Camera.TopView"),
            CameraMode::Front => t!("LevelEditor.Camera.FrontView"),
            CameraMode::Side => t!("LevelEditor.Camera.SideView"),
        }
        .to_string()
        .into_any_element()
    }

    fn dump(&self, _cx: &App) -> PanelState {
        // A split-off viewport is rebuilt by the split buttons, not restored.
        if self.split_off {
            PanelState::new(self).ephemeral()
        } else {
            PanelState::new(self)
        }
    }
}
