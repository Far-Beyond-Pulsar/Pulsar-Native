//! Renderer-backed thumbnails shared by editor asset browsers.

use std::path::Path;

/// A thumbnail renderer for one extension, contributed by another crate at
/// link time (a plugin's root crate). `ui_common` cannot depend on the crates
/// that know these formats, so they register here. Skipped while its plugin
/// is disabled.
pub struct ThumbnailRendererRegistration {
    /// The package that submitted it (`pulsar_plugins::origin!()`).
    pub origin: &'static str,
    /// Extension without the dot, lower-case.
    pub extension: &'static str,
    pub render: fn(&Path) -> Option<image::RgbaImage>,
}

impl pulsar_plugins::Contribution for ThumbnailRendererRegistration {
    const CATEGORY: &'static str = "editor.thumbnail";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.extension.to_string()
    }

    fn label(&self) -> String {
        format!(".{} thumbnails", self.extension)
    }
}

pulsar_plugins::registration!(ThumbnailRendererRegistration);

/// Register every enabled plugin's extension renderer (the meshes' models
/// and `.mesh` files, ...) with the shared engine thumbnail service. Call
/// this before requesting any asset thumbnail.
pub fn register_thumbnail_renderers() {
    for registration in pulsar_plugins::registered::<ThumbnailRendererRegistration>() {
        crate::thumbnails::register_thumbnail_renderer(registration.extension, registration.render);
    }
}
