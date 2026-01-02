use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use gpui::AppContext as _;  // Import trait for .new() method

pub struct DawEditorProvider;

impl BuiltinEditorProvider for DawEditorProvider {
    fn provider_id(&self) -> &str {
        "builtin.daw_editor"
    }
    
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("daw_project"),
                extension: "pdaw".to_string(),
                display_name: "DAW Project".to_string(),
                icon: ui::IconName::MusicNote,
                color: gpui::rgb(0x9C27B0).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!({
                    "tracks": [],
                    "tempo": 120.0,
                    "time_signature": [4, 4]
                }),
            }
        ]
    }
    
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("daw_editor"),
                display_name: "DAW Editor".to_string(),
                supported_file_types: vec![FileTypeId::new("daw_project")],
            }
        ]
    }
    
    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "daw_editor"
    }
    
    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        let editor = cx.new(|cx| crate::DawEditorPanel::new_with_project(file_path, window, cx));
        Ok(Arc::new(editor))
    }
}





