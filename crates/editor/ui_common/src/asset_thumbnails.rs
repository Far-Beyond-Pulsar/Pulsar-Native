//! Renderer-backed thumbnails shared by editor asset browsers.

use std::path::Path;

/// A thumbnail renderer for one extension, contributed by another crate at
/// link time (`pulsar_reflection::inventory::submit!`). `ui_common` cannot
/// depend on the crates that know these formats, so they register here.
pub struct ThumbnailRendererRegistration {
    /// Extension without the dot, lower-case.
    pub extension: &'static str,
    pub render: fn(&Path) -> Option<image::RgbaImage>,
}

pulsar_reflection::inventory::collect!(ThumbnailRendererRegistration);

/// Register Helio's model renderer, and every linked-in extension renderer,
/// with the shared engine thumbnail service. Call this before requesting any
/// asset thumbnail.
pub fn register_mesh_thumbnail_renderer() {
    engine_fs::thumbnails::register_mesh_thumbnail_renderer(render_mesh_thumbnail);
    for registration in pulsar_reflection::inventory::iter::<ThumbnailRendererRegistration> {
        engine_fs::thumbnails::register_thumbnail_renderer(
            registration.extension,
            registration.render,
        );
    }
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
