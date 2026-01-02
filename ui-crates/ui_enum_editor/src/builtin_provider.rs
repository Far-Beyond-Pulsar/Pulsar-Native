use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use gpui::AppContext as _;  // Import trait for .new() method

pub struct EnumEditorProvider;

impl BuiltinEditorProvider for EnumEditorProvider {
    fn provider_id(&self) -> &str {
        "builtin.enum_editor"
    }
    
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("enum"),
                extension: "enum".to_string(),
                display_name: "Enum Definition".to_string(),
                icon: ui::IconName::List,
                color: gpui::rgb(0x673AB7).into(),
                structure: FileStructure::FolderBased {
                    marker_file: "enum.json".to_string(),
                    template_structure: vec![],
                },
                default_content: serde_json::json!({
                    "name": "NewEnum",
                    "variants": []
                }),
                categories: vec!["Types".to_string()],
            }
        ]
    }
    
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("enum_editor"),
                display_name: "Enum Editor".to_string(),
                supported_file_types: vec![FileTypeId::new("enum")],
            }
        ]
    }
    
    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "enum_editor"
    }
    
    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        let actual_path = if file_path.is_dir() {
            file_path.join("enum.json")
        } else {
            file_path
        };
        
        let editor = cx.new(|cx| crate::EnumEditor::new_with_file(actual_path, window, cx));
        Ok(Arc::new(editor))
    }
}





