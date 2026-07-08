use std::path::PathBuf;

// Re-export AssetKind at crate root for convenience.
pub use ui_types_common::AssetKind;

// ============================================================================
// Cross-crate asset actions
// ============================================================================

/// Dispatch this action to open a file or directory in its default editor.
///
/// Any crate that has `plugin_editor_api` as a dependency can dispatch this
/// action; the application layer (`ui_core`) registers the handler that routes
/// it to the correct in-engine editor or falls back to the OS default.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, schemars::JsonSchema, gpui::Action)]
#[action(namespace = pulsar_app)]
pub struct OpenAsset {
    pub path: PathBuf,
}
