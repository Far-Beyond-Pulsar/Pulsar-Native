//! Viewport dock panel.

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::ui::ViewportPanel;
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
}

impl ViewportPanelWrapper {
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
        }
    }
}

impl EventEmitter<PanelEvent> for ViewportPanelWrapper {}

ui_common::panel_boilerplate!(ViewportPanelWrapper);

impl Render for ViewportPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("viewport panel: render");
        let _t = gpui::render_stats::scope("viewport panel: render");

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
