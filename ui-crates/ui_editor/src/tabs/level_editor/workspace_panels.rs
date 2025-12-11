//! Workspace panels for Level Editor

use gpui::*;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use super::ui::{SceneBrowser, HierarchyPanel, PropertiesPanel, ViewportPanel, LevelEditorState};
use std::sync::Arc;
use std::rc::Rc;
use std::cell::RefCell;
use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::GameThread;

/// Scene Browser Panel
pub struct SceneBrowserPanel {
    scene_browser: SceneBrowser,
    focus_handle: FocusHandle,
}

impl SceneBrowserPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            scene_browser: SceneBrowser::new(),
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for SceneBrowserPanel {}

impl Render for SceneBrowserPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.scene_browser.render(cx))
    }
}

impl Focusable for SceneBrowserPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SceneBrowserPanel {
    fn panel_name(&self) -> &'static str {
        "scene_browser"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Scenes".into_any_element()
    }
}

/// Hierarchy Panel
pub struct HierarchyPanelWrapper {
    hierarchy: HierarchyPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
}

impl HierarchyPanelWrapper {
    pub fn new(state: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut Context<Self>) -> Self {
        Self {
            hierarchy: HierarchyPanel::new(),
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for HierarchyPanelWrapper {}

impl Render for HierarchyPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_1()
            .child(self.hierarchy.render(&*state, cx))
    }
}

impl Focusable for HierarchyPanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for HierarchyPanelWrapper {
    fn panel_name(&self) -> &'static str {
        "hierarchy"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Hierarchy".into_any_element()
    }
}

/// Properties Panel
pub struct PropertiesPanelWrapper {
    properties: PropertiesPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
}

impl PropertiesPanelWrapper {
    pub fn new(state: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut Context<Self>) -> Self {
        Self {
            properties: PropertiesPanel::new(),
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for PropertiesPanelWrapper {}

impl Render for PropertiesPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.properties.render(&*state, cx))
    }
}

impl Focusable for PropertiesPanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for PropertiesPanelWrapper {
    fn panel_name(&self) -> &'static str {
        "properties"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Properties".into_any_element()
    }
}

/// Viewport Panel Wrapper
pub struct ViewportPanelWrapper {
    viewport_panel: ViewportPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    fps_graph_is_line: Rc<RefCell<bool>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    game_thread: Arc<GameThread>,
    focus_handle: FocusHandle,
}

impl ViewportPanelWrapper {
    pub fn new(
        viewport_panel: ViewportPanel,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        fps_graph_is_line: Rc<RefCell<bool>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        game_thread: Arc<GameThread>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            viewport_panel,
            state,
            fps_graph_is_line,
            gpu_engine,
            game_thread,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for ViewportPanelWrapper {}

impl Render for ViewportPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut state = self.state.write();
        div()
            .size_full()
            .p_1()
            .child(
                self.viewport_panel.render(
                    &mut *state,
                    self.state.clone(),
                    self.fps_graph_is_line.clone(),
                    &self.gpu_engine,
                    &self.game_thread,
                    cx
                )
            )
    }
}

impl Focusable for ViewportPanelWrapper {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
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
