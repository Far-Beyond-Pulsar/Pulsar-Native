//! The asset-path property types of what this crate owns. It has no GPUI code,
//! so `ui_common` registers their pickers.

pulsar_world_registry::asset_path_type! {
    /// An image file, project-relative: what [`crate::texture_assets`] loads.
    pub TexturePath,
    editor = elsewhere,
    extensions = ["png", "jpg", "jpeg", "tga", "bmp", "gif", "webp"]
}

pulsar_world_registry::asset_path_type! {
    /// A level file, project-relative.
    pub LevelPath,
    editor = elsewhere,
    extensions = ["level"]
}
