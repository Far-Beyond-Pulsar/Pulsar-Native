//! Build Domain — editor-side build bookkeeping.
//!
//! The build configuration, target platform, build mode and the running game
//! process are engine-global (`engine_state::playback`); only what is specific
//! to the level editor lives here.

use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// How a screenshot taken by the `viewport_screenshot` MCP tool is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenshotFormat {
    Png,
    /// JPEG at this quality, 1 to 100.
    Jpeg(u8),
}

/// What the viewport reports back to the tool that asked for a screenshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpScreenshotOutcome {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub format: ScreenshotFormat,
    pub bytes: u64,
    /// GPU tiles the frame was rendered in.
    pub tiles: u64,
    /// From the viewport taking the request to the file being written.
    pub millis: u64,
}

/// A screenshot the `viewport_screenshot` MCP tool asked for. The viewport
/// takes it on its next frame, renders it on an editor task and answers on
/// `reply`; no dialog is involved.
#[derive(Clone, Debug)]
pub struct McpScreenshotRequest {
    pub width: u32,
    pub height: u32,
    pub destination: PathBuf,
    pub format: ScreenshotFormat,
    pub hide_grid: bool,
    pub hide_gizmos: bool,
    pub hide_overlays: bool,
    pub reply: Sender<Result<McpScreenshotOutcome, String>>,
}

/// Level-editor build domain.
#[derive(Clone, Default)]
pub struct BuildDomain {
    /// When set, the viewport should capture its framebuffer to this path on
    /// the next render frame.
    pub pending_thumbnail_capture: Option<PathBuf>,
    /// One-shot high-resolution capture requested by the screenshot tool.
    pub pending_screenshot_size: Option<(u32, u32)>,
    /// A screenshot requested through MCP, taken by the viewport on its next
    /// frame (see [`McpScreenshotRequest`]).
    pub pending_mcp_screenshot: Option<McpScreenshotRequest>,
    /// Last physical Helio viewport resolution, used by multiplier captures.
    pub viewport_size: (u32, u32),
    /// Device-reported maximum side length for a 2D texture.
    pub max_texture_dimension_2d: u32,
    /// True while the viewport is rendering and reading back a screenshot.
    pub screenshot_processing: bool,
    /// Completed screenshot tiles and total tile count while capture runs.
    pub screenshot_progress: Option<(u64, u64)>,
}
