use crate::AssetKind;
use std::path::Path;

/// The canonical drag payload for any asset dragged out of the file manager
/// drawer or any other engine panel.
///
/// # Usage — emitting a drag
///
/// ```rust,ignore
/// .on_drag(AssetPayload::from_path(&item.path), |drag, _, _, cx| {
///     cx.new(|_| drag.clone())
/// })
/// ```
///
/// # Usage — receiving a drop (via [`AssetDropArea`])
///
/// ```rust,ignore
/// AssetDropArea::new("level-viewport")
///     .accepts(vec![AssetKind::Mesh, AssetKind::Scene])
///     .on_asset_drop(cx.listener(|this, payload, window, cx| {
///         this.spawn_asset(payload, window, cx);
///     }))
///     .child(viewport_content)
/// ```
#[derive(Clone, Debug)]
pub struct AssetPayload {
    /// Engine-FS relative path using forward-slash separators.
    /// May be an absolute OS path when the file has not yet been imported.
    pub engine_path: String,

    /// File name (without parent directory), used in the drag ghost and toasts.
    pub name: String,

    /// Semantic kind, derived from the file extension.
    pub kind: AssetKind,

    /// Lowercase extension without the leading dot (e.g. `"fbx"`).
    pub extension: String,
}

impl AssetPayload {
    /// Build a payload from any filesystem path. Kind is inferred from the
    /// extension. For best results, pass the engine-FS relative path.
    pub fn from_path(path: &Path) -> Self {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();
        let extension = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let kind = AssetKind::from_extension(&extension);
        // Normalise to forward slashes for engine-FS compatibility.
        let engine_path = path.to_string_lossy().replace('\\', "/");
        Self {
            engine_path,
            name,
            kind,
            extension,
        }
    }
}
