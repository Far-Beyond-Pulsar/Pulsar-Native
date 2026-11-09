//! The catalog fixture plugin's editor code. Registers nothing: its root
//! crate does.

/// The module of its assistant tools.
pub const TOOLS: &str = module_path!();

/// Its thumbnails (none).
pub fn render_thumbnail(_: &std::path::Path) -> Option<image::RgbaImage> {
    None
}
