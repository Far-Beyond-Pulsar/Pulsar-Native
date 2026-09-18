use super::*;

impl LevelEditorPanel {
    pub(in crate::level_editor::ui::panel) fn on_toggle_grid(&mut self, _: &ToggleGrid, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_grid();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_wireframe(&mut self, _: &ToggleWireframe, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_wireframe();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_lighting(&mut self, _: &ToggleLighting, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().editor.toggle_lighting();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_performance_overlay(
        &mut self,
        _: &TogglePerformanceOverlay,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_performance_overlay();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_camera_mode_selector(
        &mut self,
        _: &ToggleCameraModeSelector,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_camera_mode_selector();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_viewport_options(
        &mut self,
        _: &ToggleViewportOptions,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_viewport_options();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_fps_graph_type(
        &mut self,
        _: &ToggleFpsGraphType,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_fps_graph_type();
        cx.notify();
    }

    // Performance metrics toggles
    pub(in crate::level_editor::ui::panel) fn on_toggle_fps_graph(&mut self, _: &ToggleFpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().overlays.toggle_fps_graph();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_tps_graph(&mut self, _: &ToggleTpsGraph, _: &mut Window, cx: &mut Context<Self>) {
        self.shared_state.write().overlays.toggle_tps_graph();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_frame_time_graph(
        &mut self,
        _: &ToggleFrameTimeGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_frame_time_graph();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_memory_graph(
        &mut self,
        _: &ToggleMemoryGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_memory_graph();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_draw_calls_graph(
        &mut self,
        _: &ToggleDrawCallsGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_draw_calls_graph();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_vertices_graph(
        &mut self,
        _: &ToggleVerticesGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state.write().overlays.toggle_vertices_graph();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_input_latency_graph(
        &mut self,
        _: &ToggleInputLatencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_input_latency_graph();
        cx.notify();
    }

    pub(in crate::level_editor::ui::panel) fn on_toggle_ui_consistency_graph(
        &mut self,
        _: &ToggleUiConsistencyGraph,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.shared_state
            .write()
            .overlays
            .toggle_ui_consistency_graph();
        cx.notify();
    }

}
