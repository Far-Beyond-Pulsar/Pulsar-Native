use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use gpui::AppContext as _;  // Import trait for .new() method

pub struct AliasEditorProvider;

impl BuiltinEditorProvider for AliasEditorProvider {
    fn provider_id(&self) -> &str {
        "builtin.alias_editor"
    }
    
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("alias"),
                extension: "alias".to_string(),
                display_name: "Type Alias".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0x3F51B5).into(),
                structure: FileStructure::FolderBased {
                    marker_file: "alias.json".to_string(),
                    template_structure: vec![],
                },
                default_content: serde_json::json!({
                    "name": "NewAlias",
                    "target": "i32"
                }),
            }
        ]
    }
    
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("alias_editor"),
                display_name: "Alias Editor".to_string(),
                supported_file_types: vec![FileTypeId::new("alias")],
            }
        ]
    }
    
    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "alias_editor"
    }
    
    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        let actual_path = if file_path.is_dir() {
            file_path.join("alias.json")
        } else {
            file_path
        };
        
        let editor = cx.new(|cx| crate::AliasEditor::new_with_file(actual_path, window, cx));
        Ok(Arc::new(editor))
    }
}





