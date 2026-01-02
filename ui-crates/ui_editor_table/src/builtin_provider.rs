use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use gpui::AppContext as _;  // Import trait for .new() method

pub struct TableEditorProvider;

impl BuiltinEditorProvider for TableEditorProvider {
    fn provider_id(&self) -> &str {
        "builtin.table_editor"
    }
    
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("database"),
                extension: "db".to_string(),
                display_name: "SQLite Database (.db)".to_string(),
                icon: FileIcon::Database,
                structure: FileStructure::Standalone,
                default_content: serde_json::Value::Null,
            },
            FileTypeDefinition {
                id: FileTypeId::new("sqlite"),
                extension: "sqlite".to_string(),
                display_name: "SQLite Database (.sqlite)".to_string(),
                icon: FileIcon::Database,
                structure: FileStructure::Standalone,
                default_content: serde_json::Value::Null,
            },
            FileTypeDefinition {
                id: FileTypeId::new("sqlite3"),
                extension: "sqlite3".to_string(),
                display_name: "SQLite Database (.sqlite3)".to_string(),
                icon: FileIcon::Database,
                structure: FileStructure::Standalone,
                default_content: serde_json::Value::Null,
            },
        ]
    }
    
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("table_editor"),
                display_name: "Table Editor".to_string(),
                supported_file_types: vec![
                    FileTypeId::new("database"),
                    FileTypeId::new("sqlite"),
                    FileTypeId::new("sqlite3"),
                ],
            }
        ]
    }
    
    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "table_editor"
    }
    
    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        let editor = cx.new(|cx| {
            crate::DataTableEditor::open_database(file_path, window, cx)
                .unwrap_or_else(|e| {
                    tracing::error!("Failed to open database: {}", e);
                    crate::DataTableEditor::new(window, cx)
                })
        });
        Ok(Arc::new(editor))
    }
}





