//! The "READ ONLY" watermark editors draw over a graph canvas that opens a
//! file they cannot write (a plugin-served example, see
//! `engine_fs::virtual_fs::mount_of`).

use gpui::prelude::*;
use gpui::*;

/// Whether `path` is served read-only (a plugin's folder or the `plugins/` mount).
pub fn is_read_only_path(path: &std::path::Path) -> bool {
    engine_fs::virtual_fs::mount_of(path).is_some()
}

/// A large, faint "READ ONLY" in the top-right corner of the nearest
/// positioned (`relative`) parent. It never takes mouse input.
pub fn read_only_watermark() -> AnyElement {
    div()
        .absolute()
        .top(px(8.0))
        .right(px(16.0))
        .text_size(px(48.0))
        .font_weight(FontWeight::BOLD)
        .text_color(hsla(0.0, 0.0, 0.55, 0.35))
        .child("READ-ONLY")
        .into_any_element()
}
