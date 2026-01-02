use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use gpui::AppContext as _;  // Import trait for .new() method

pub struct StructEditorProvider;

impl BuiltinEditorProvider for StructEditorProvider {
    fn provider_id(&self) -> &str {
        "builtin.struct_editor"
    }
    
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("struct"),
                extension: "struct".to_string(),
                display_name: "Struct Definition".to_string(),
                icon: ui::IconName::Box,
                color: gpui::rgb(0x00BCD4).into(),
                structure: FileStructure::FolderBased {
                    marker_file: "struct.json".to_string(),
                    template_structure: vec![],
                },
                default_content: serde_json::json!({
                    "name": "NewStruct",
                    "fields": []
                }),
                categories: vec!["Types".to_string()],
            }
        ]
    }
    
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("struct_editor"),
                display_name: "Struct Editor".to_string(),
                supported_file_types: vec![FileTypeId::new("struct")],
            }
        ]
    }
    
    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "struct_editor"
    }
    
    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        let actual_path = if file_path.is_dir() {
            file_path.join("struct.json")
        } else {
            file_path
        };
        
        let editor = cx.new(|cx| crate::StructEditor::new_with_file(actual_path, window, cx));
        Ok(Arc::new(editor))
    }
}



