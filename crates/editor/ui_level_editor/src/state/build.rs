//! Build Domain — editor-side build bookkeeping.
//!
//! The build configuration, target platform, build mode and the running game
//! process are engine-global (`engine_state::playback`); only what is specific
//! to the level editor lives here.

use std::path::PathBuf;

/// Level-editor build domain.
#[derive(Clone, Default)]
pub struct BuildDomain {
    /// When set, the viewport should capture its framebuffer to this path on
    /// the next render frame.
    pub pending_thumbnail_capture: Option<PathBuf>,
    /// One-shot high-resolution capture requested by the screenshot tool.
    pub pending_screenshot_size: Option<(u32, u32)>,
    /// Last physical Helio viewport resolution, used by multiplier captures.
    pub viewport_size: (u32, u32),
    /// Device-reported maximum side length for a 2D texture.
    pub max_texture_dimension_2d: u32,
    /// True while the viewport is rendering and reading back a screenshot.
    pub screenshot_processing: bool,
    /// Completed screenshot tiles and total tile count while capture runs.
    pub screenshot_progress: Option<(u64, u64)>,
}
