use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use gpui::AppContext as _;  // Import trait for .new() method

pub struct TraitEditorProvider;

impl BuiltinEditorProvider for TraitEditorProvider {
    fn provider_id(&self) -> &str {
        "builtin.trait_editor"
    }
    
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("trait"),
                extension: "trait".to_string(),
                display_name: "Trait Definition".to_string(),
                icon: FileIcon::Trait,
                structure: FileStructure::FolderBased {
                    marker_file: "trait.json".to_string(),
                    template_structure: vec![],
                },
                default_content: serde_json::json!({
                    "name": "NewTrait",
                    "methods": []
                }),
            }
        ]
    }
    
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("trait_editor"),
                display_name: "Trait Editor".to_string(),
                supported_file_types: vec![FileTypeId::new("trait")],
            }
        ]
    }
    
    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "trait_editor"
    }
    
    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        let actual_path = if file_path.is_dir() {
            file_path.join("trait.json")
        } else {
            file_path
        };
        
        let editor = cx.new(|cx| crate::TraitEditor::new_with_file(actual_path, window, cx));
        Ok(Arc::new(editor))
    }
}





