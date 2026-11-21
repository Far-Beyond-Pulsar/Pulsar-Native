//! The editor provider: the `.mocap` file type and the editor that opens it.

use crate::editor::MocapEditor;
use gpui::{App, AppContext as _, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::PanelView;

/// The provider's id, and the plugin editor's.
pub const PROVIDER_ID: &str = "com.pulsar.mocap-editor";
const EDITOR_ID: &str = "mocap-editor";

/// Opens `.mocap` takes in the mocap editor.
pub struct MocapBuiltinProvider;

impl MocapBuiltinProvider {
    /// The registration's factory.
    pub fn create() -> Arc<dyn BuiltinEditorProvider> {
        Arc::new(Self)
    }
}

impl BuiltinEditorProvider for MocapBuiltinProvider {
    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![FileTypeDefinition {
            id: FileTypeId::new(mocap_format::EXTENSION),
            extension: mocap_format::EXTENSION.to_owned(),
            display_name: "Motion Capture Take".to_owned(),
            icon: ui::IconName::VideoCamera,
            color: gpui::rgb(0xE5484D).into(),
            structure: FileStructure::Standalone,
            // Placeholder JSON (an empty string would hide the type from the Create menu).
            // A file without the take magic is a new session; recording replaces it.
            default_content: json!({ "mocap_session": 1 }),
            creation_directory: None,
            cook: plugin_editor_api::file_types::copy_file_for_cook,
            categories: vec!["Animation".to_owned()],
        }]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new(EDITOR_ID),
            display_name: "Mocap Editor".into(),
            supported_file_types: vec![FileTypeId::new(mocap_format::EXTENSION)],
        }]
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == EDITOR_ID
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        _window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let panel = cx.new(|cx| MocapEditor::new(file_path, cx));
        Ok(Arc::new(panel))
    }
}
