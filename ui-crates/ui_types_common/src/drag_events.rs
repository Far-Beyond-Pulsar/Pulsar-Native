use plugin_editor_api::AssetPayload;

/// Global drag-and-drop lifecycle events.
///
/// These events are emitted by draggable items and can be observed by the
/// application to implement cross-panel drag behaviors like drawer auto-close.
#[derive(Clone, Debug)]
pub enum DragEvent {
    /// Emitted when a user starts dragging an asset via a drag handle.
    ///
    /// Typical use: close the file manager drawer to maximize viewport space.
    AssetDragStarted(AssetPayload),

    /// Emitted when an asset is successfully dropped on a valid drop target.
    ///
    /// This can be used for analytics or to update UI state.
    AssetDropped(AssetPayload),

    /// Emitted when a drag is cancelled (ESC key or drop on invalid target).
    ///
    /// Typical use: reopen the file manager drawer if it was auto-closed.
    AssetDragCancelled,
}
