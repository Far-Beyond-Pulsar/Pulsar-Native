//! Overlay Domain — viewport overlay visibility, collapse state, positions, and
//! performance metric toggles.
//!
//! This domain collects all the "chrome" state that controls which floating
//! panels are visible in the viewport, where they are positioned, and which
//! performance graphs are active.
//!
//! Separating this from [`EditorDomain`](super::editor::EditorDomain) keeps
//! rendering preferences (grid, wireframe, lighting) distinct from overlay
//! chrome, making both easier to reason about.

use std::collections::HashSet;

/// Viewport overlay positions and drag state.
#[derive(Clone, Debug)]
pub struct OverlayPositions {
    /// Camera selector position relative to bottom-left corner.
    pub camera: (f32, f32),
    /// Viewport options position relative to top-left corner.
    pub viewport: (f32, f32),
    /// Whether the camera selector is being dragged.
    pub is_dragging_camera: bool,
    /// Whether the viewport options is being dragged.
    pub is_dragging_viewport: bool,
    /// Mouse position when the camera overlay drag started.
    pub camera_drag_start: Option<(f32, f32)>,
    /// Mouse position when the viewport overlay drag started.
    pub viewport_drag_start: Option<(f32, f32)>,
}

impl Default for OverlayPositions {
    fn default() -> Self {
        Self {
            camera: (16.0, 16.0),
            viewport: (16.0, 16.0),
            is_dragging_camera: false,
            is_dragging_viewport: false,
            camera_drag_start: None,
            viewport_drag_start: None,
        }
    }
}

// ── Overlay collapse state ────────────────────────────────────────────────

/// Which viewport overlays have their collapse state and which perf graphs are shown.
#[derive(Clone)]
pub struct OverlayState {
    // ── Overlay visibility ─────────────────────────────────────────────
    pub show_performance_overlay: bool,
    pub show_gpu_pipeline_overlay: bool,
    pub show_camera_mode_selector: bool,
    pub show_viewport_options: bool,

    // ── Collapse state (when X is clicked, overlay collapses to a button) ─
    pub camera_mode_selector_collapsed: bool,
    pub viewport_options_collapsed: bool,
    pub performance_overlay_collapsed: bool,
    pub gpu_pipeline_overlay_collapsed: bool,

    // ── FPS graph type ─────────────────────────────────────────────────
    pub fps_graph_is_line: bool,

    // ── Individual metric toggles ──────────────────────────────────────
    pub show_fps_graph: bool,
    pub show_tps_graph: bool,
    pub show_frame_time_graph: bool,
    pub show_memory_graph: bool,
    pub show_draw_calls_graph: bool,
    pub show_vertices_graph: bool,
    pub show_input_latency_graph: bool,
    pub show_ui_consistency_graph: bool,
}

impl Default for OverlayState {
    fn default() -> Self {
        Self {
            show_performance_overlay: false,
            show_gpu_pipeline_overlay: false,
            show_camera_mode_selector: true,
            show_viewport_options: true,
            camera_mode_selector_collapsed: false,
            viewport_options_collapsed: false,
            performance_overlay_collapsed: false,
            gpu_pipeline_overlay_collapsed: false,
            fps_graph_is_line: true,
            // Default: Frame Time, GPU Memory, Input Latency
            show_fps_graph: false,
            show_tps_graph: false,
            show_frame_time_graph: true,
            show_memory_graph: true,
            show_draw_calls_graph: false,
            show_vertices_graph: false,
            show_input_latency_graph: true,
            show_ui_consistency_graph: false,
        }
    }
}

// ── Overlay domain ────────────────────────────────────────────────────────

/// Overlay UI domain — controls which floating panels and graphs are visible.
#[derive(Clone)]
pub struct OverlayDomain {
    /// Overlay toggle and collapse states.
    pub state: OverlayState,
    /// Overlay pixel positions and drag state.
    pub positions: OverlayPositions,
}

impl Default for OverlayDomain {
    fn default() -> Self {
        Self {
            state: OverlayState::default(),
            positions: OverlayPositions::default(),
        }
    }
}

// ── Convenience methods that forward into `state` ─────────────────────────

impl OverlayDomain {
    // ── Visibility toggles ────────────────────────────────────────────────

    pub fn toggle_performance_overlay(&mut self) {
        self.state.show_performance_overlay = !self.state.show_performance_overlay;
    }

    pub fn toggle_camera_mode_selector(&mut self) {
        self.state.show_camera_mode_selector = !self.state.show_camera_mode_selector;
    }

    pub fn toggle_viewport_options(&mut self) {
        self.state.show_viewport_options = !self.state.show_viewport_options;
    }

    // ── Collapse toggles ──────────────────────────────────────────────────

    pub fn set_camera_mode_selector_collapsed(&mut self, collapsed: bool) {
        self.state.camera_mode_selector_collapsed = collapsed;
    }

    pub fn set_viewport_options_collapsed(&mut self, collapsed: bool) {
        self.state.viewport_options_collapsed = collapsed;
    }

    pub fn set_performance_overlay_collapsed(&mut self, collapsed: bool) {
        self.state.performance_overlay_collapsed = collapsed;
    }

    pub fn set_gpu_pipeline_overlay_collapsed(&mut self, collapsed: bool) {
        self.state.gpu_pipeline_overlay_collapsed = collapsed;
    }

    // ── FPS graph type ────────────────────────────────────────────────────

    pub fn toggle_fps_graph_type(&mut self) {
        self.state.fps_graph_is_line = !self.state.fps_graph_is_line;
    }

    // ── Individual metric toggles ─────────────────────────────────────────

    pub fn toggle_fps_graph(&mut self) {
        self.state.show_fps_graph = !self.state.show_fps_graph;
    }

    pub fn toggle_tps_graph(&mut self) {
        self.state.show_tps_graph = !self.state.show_tps_graph;
    }

    pub fn toggle_frame_time_graph(&mut self) {
        self.state.show_frame_time_graph = !self.state.show_frame_time_graph;
    }

    pub fn toggle_memory_graph(&mut self) {
        self.state.show_memory_graph = !self.state.show_memory_graph;
    }

    pub fn toggle_draw_calls_graph(&mut self) {
        self.state.show_draw_calls_graph = !self.state.show_draw_calls_graph;
    }

    pub fn toggle_vertices_graph(&mut self) {
        self.state.show_vertices_graph = !self.state.show_vertices_graph;
    }

    pub fn toggle_input_latency_graph(&mut self) {
        self.state.show_input_latency_graph = !self.state.show_input_latency_graph;
    }

    pub fn toggle_ui_consistency_graph(&mut self) {
        self.state.show_ui_consistency_graph = !self.state.show_ui_consistency_graph;
    }

    // ── Setter methods (for bindings) ──────────────────────────────────────

    pub fn set_show_performance_overlay(&mut self, show: bool) {
        self.state.show_performance_overlay = show;
    }

    pub fn set_show_gpu_pipeline_overlay(&mut self, show: bool) {
        self.state.show_gpu_pipeline_overlay = show;
    }

    pub fn set_show_camera_mode_selector(&mut self, show: bool) {
        self.state.show_camera_mode_selector = show;
    }

    pub fn set_show_viewport_options(&mut self, show: bool) {
        self.state.show_viewport_options = show;
    }

    pub fn set_show_grid(&mut self, show: bool) {
        // Grid is in EditorDomain, not here — so this method remains
        // for callers that pass state generically.
    }
}
