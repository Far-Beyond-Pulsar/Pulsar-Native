//! The plugin's editor provider: its file types and editors, offered to
//! the editor while the plugin is enabled (`plugin_manager::PluginEditorProvider`).

use gpui::AppContext;
use gpui::{App, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::PanelView;

/// Wraps the asset_viewer_plugin crate as a built-in editor provider.
/// Opens `.fbx` and `.png` files in the FBX/PNG Asset Viewer.
pub struct AssetViewerBuiltinProvider;

impl BuiltinEditorProvider for AssetViewerBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.asset-viewer"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        crate::AssetViewerPlugin::default().file_types()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        crate::AssetViewerPlugin::default().editors()
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "asset-viewer"
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let panel = cx.new(|cx| crate::AssetViewerPanel::new(file_path.clone(), window, cx));
        let undo = editor_context.services.undo.clone();
        panel.update(cx, |panel, _| panel.set_undo_service(undo));
        Ok(Arc::new(panel))
    }
}

plugin_manager::inventory::submit! {
    plugin_manager::PluginEditorProvider {
        origin: pulsar_plugins::origin!(),
        id: "com.pulsar.asset-viewer",
        create: || Arc::new(AssetViewerBuiltinProvider),
    }
}
