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
}
