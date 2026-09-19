//! Layout and visual constants for the flamegraph viewer

pub const ROW_HEIGHT: f32 = 26.0;
pub const MIN_SPAN_WIDTH: f32 = 2.0;
pub const PADDING: f32 = 2.0;
pub const GRAPH_HEIGHT: f32 = 100.0;
pub const THREAD_LABEL_WIDTH: f32 = 120.0;
pub const THREAD_ROW_PADDING: f32 = 30.0;
pub const TIMELINE_HEIGHT: f32 = 90.0;
pub const STATS_SIDEBAR_WIDTH: f32 = 250.0;
pub const TITLE_BAR_HEIGHT: f32 = 34.0;
pub const CULL_PADDING: f32 = 100.0;

/// Minimum on-screen spacing (px) between frame boundary lines. Below this the
/// lines are culled to avoid a noisy solid wall at far zoom-out.
pub const FRAME_LINE_MIN_SPACING: f32 = 3.0;
/// Background alpha of the frame boundary lines.
pub const FRAME_LINE_COLOR: [f32; 4] = [0.95, 0.72, 0.25, 0.32];
