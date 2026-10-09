//! Renderer-backed thumbnails shared by editor asset browsers.

use std::path::Path;

/// Register Helio's model renderer with the shared engine thumbnail service.
/// Call this before requesting any asset thumbnail.
pub fn register_mesh_thumbnail_renderer() {
    engine_fs::thumbnails::register_mesh_thumbnail_renderer(render_mesh_thumbnail);
}

fn render_mesh_thumbnail(path: &Path) -> Option<image::RgbaImage> {
    match helio_snapshot::render_preview(
        path,
        helio_snapshot::SnapshotConfig {
            width: 128,
            height: 128,
            fit_margin: 1.12,
            ..Default::default()
        },
    ) {
        Ok(image) => Some(image),
        Err(error) => {
            tracing::warn!("mesh thumbnail render failed for {:?}: {error}", path);
            None
        }
    }
}
