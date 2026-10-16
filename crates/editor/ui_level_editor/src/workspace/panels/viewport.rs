//! Viewport dock panel.

use crate::state::LevelEditorState;
use crate::ui::ViewportPanel;
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use std::sync::Arc;
use ui::dock::{Panel, PanelEvent};

/// Viewport Panel Wrapper
pub struct ViewportPanelWrapper {
    viewport_panel: ViewportPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    focus_handle: FocusHandle,
    pump_started: bool,
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
        }
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
            crate::ui::frame_pump::spawn_frame_pump(&cx.entity(), window, move |this, _, cx| {
                let next = this
                    .viewport_panel
                    .controls_signature(&this.state.read(), cx);
                if next != last {
                    last = next;
                    cx.notify();
                }
            });
        }

        let state = self.state.read();
        self.viewport_panel
            .render(&state, self.state.clone(), &self.gpu_engine, cx)
    }
}

impl Panel for ViewportPanelWrapper {
    fn panel_name(&self) -> &'static str {
        "viewport"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Viewport".into_any_element()
    }
}
