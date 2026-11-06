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

/// Register every linked-in extension renderer (the meshes' models and
/// `.mesh` files, ...) with the shared engine thumbnail service. Call this
/// before requesting any asset thumbnail.
pub fn register_thumbnail_renderers() {
    for registration in pulsar_reflection::inventory::iter::<ThumbnailRendererRegistration> {
        engine_fs::thumbnails::register_thumbnail_renderer(
            registration.extension,
            registration.render,
        );
    }
}
