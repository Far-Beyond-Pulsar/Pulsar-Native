use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use gpui::AppContext as _;  // Import trait for .new() method

pub struct ScriptEditorProvider;

impl BuiltinEditorProvider for ScriptEditorProvider {
    fn provider_id(&self) -> &str {
        "builtin.script_editor"
    }
    
    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("rust_script"),
                extension: "rs".to_string(),
                display_name: "Rust".to_string(),
                icon: ui::IconName::RustLang,
                color: gpui::rgb(0xFF5722).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!("// New Rust script\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("javascript"),
                extension: "js".to_string(),
                display_name: "JavaScript".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0xF7DF1E).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!("// New JavaScript file\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("typescript"),
                extension: "ts".to_string(),
                display_name: "TypeScript".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0x3178C6).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!("// New TypeScript file\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("python"),
                extension: "py".to_string(),
                display_name: "Python Script".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0x3776AB).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!("# New Python script\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("lua"),
                extension: "lua".to_string(),
                display_name: "Lua Script".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0x2196F3).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!("-- New Lua script\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("toml"),
                extension: "toml".to_string(),
                display_name: "TOML Configuration".to_string(),
                icon: ui::IconName::Page,
                color: gpui::rgb(0x9E9E9E).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!("# TOML configuration file\n"),
                categories: vec!["Data".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("markdown"),
                extension: "md".to_string(),
                display_name: "Markdown Document".to_string(),
                icon: ui::IconName::Page,
                color: gpui::rgb(0xFF5722).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!("# New Document\n"),
                categories: vec!["Documents".to_string()],
            },
        ]
    }
    
    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new("script_editor"),
                display_name: "Script Editor".to_string(),
                supported_file_types: vec![
                    FileTypeId::new("rust_script"),
                    FileTypeId::new("javascript"),
                    FileTypeId::new("typescript"),
                    FileTypeId::new("python"),
                    FileTypeId::new("lua"),
                    FileTypeId::new("toml"),
                    FileTypeId::new("markdown"),
                ],
            }
        ]
    }
    
    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "script_editor"
    }
    
    fn create_editor(
        &self,
        file_path: PathBuf,
        editor_context: &EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        let editor = cx.new(|cx| crate::ScriptEditorPanel::new(window, cx));
        
        // Use the provided context to set project path
        if let Some(project_root) = &editor_context.project_root {
            editor.update(cx, |ed, ecx| {
                ed.set_project_path(project_root.clone(), window, ecx);
                ed.open_file(file_path, window, ecx);
            });
        } else {
            editor.update(cx, |ed, ecx| {
                ed.open_file(file_path, window, ecx);
            });
        }
        
        Ok(Arc::new(editor))
    }
}