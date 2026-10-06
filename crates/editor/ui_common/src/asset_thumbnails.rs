//! Renderer-backed thumbnails shared by editor asset browsers.

use std::path::Path;

static MESH_RENDER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Register Helio's model renderer with the shared engine thumbnail service.
/// Call this before requesting any asset thumbnail.
pub fn register_mesh_thumbnail_renderer() {
    engine_fs::thumbnails::register_mesh_thumbnail_renderer(render_mesh_thumbnail);
}

fn render_mesh_thumbnail(path: &Path) -> Option<image::RgbaImage> {
    let _render_guard = MESH_RENDER_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    match helio_snapshot::render_snapshot(
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
