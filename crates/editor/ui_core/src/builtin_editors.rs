//! Registration of the editors: the Level Editor, which is the editor's own,
//! and every linked one. Each plugin's editors (the code editor, the asset
//! viewer, animation, physics, input, widgets, tables, Matter, Blueprints,
//! render graphs, materials) are its own provider, submitted from its crate
//! (`plugin_manager::PluginEditorProvider`, gated by the plugin's
//! enablement); nothing here names a plugin.

use gpui::AppContext;
use gpui::{App, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dock::PanelView;

// ---------------------------------------------------------------------------
// Level Editor — built-in provider
// ---------------------------------------------------------------------------

/// Opens `.level` and `.level.json` files in the Level Editor panel.
pub struct LevelEditorBuiltinProvider;

impl BuiltinEditorProvider for LevelEditorBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.level-editor"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("level"),
                extension: "level".to_string(),
                display_name: "Pulsar Level".to_string(),
                icon: ui::IconName::Map,
                color: gpui::rgb(0x4CAF50).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!({
                    "version": "2.0",
                    "objects": [],
                    "metadata": {
                        "created": "",
                        "modified": "",
                        "editor_version": ""
                    }
                }),
                creation_directory: None,

                categories: vec!["Levels".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("level.json"),
                extension: "level.json".to_string(),
                display_name: "Pulsar Level (JSON)".to_string(),
                icon: ui::IconName::Map,
                color: gpui::rgb(0x4CAF50).into(),
                structure: FileStructure::Standalone,
                default_content: serde_json::json!({
                    "version": "2.0",
                    "objects": [],
                    "metadata": {
                        "created": "",
                        "modified": "",
                        "editor_version": ""
                    }
                }),
                creation_directory: None,

                categories: vec!["Levels".to_string()],
            },
        ]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("level-editor"),
            display_name: "Level Editor".into(),
            supported_file_types: vec![FileTypeId::new("level"), FileTypeId::new("level.json")],
        }]
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "level-editor"
    }

    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        ui_level_editor::ai_tools::ai_tools()
    }

    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        let is_level_file = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(".level") || name.ends_with(".level.json"))
            .unwrap_or(false);

        if is_level_file {
            ui_level_editor::ai_tools::capabilities_for_file(file_path)
        } else {
            Vec::new()
        }
    }

    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        tool_args: JsonValue,
    ) -> Result<JsonValue, PluginError> {
        ui_level_editor::ai_tools::execute_ai_tool(file_path, tool_name, tool_args)
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let undo = editor_context.services.undo.clone();
        let panel = cx.new(|cx| {
            let mut panel = match ui_level_editor::LevelEditorPanel::new_with_path(
                file_path.clone(),
                window,
                cx,
            ) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("Failed to load level {:?}: {}", file_path, e);
                    ui_level_editor::LevelEditorPanel::new(window, cx)
                }
            };
            panel.set_undo_service(undo);
            panel
        });
        Ok(Arc::new(panel))
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register all built-in editors with the registry.
///
/// This should be called during application initialization,
/// before any files are opened.
pub fn register_all_builtin_editors(registry: &mut BuiltinEditorRegistry) {
    tracing::info!("Registering all built-in editors...");

    // Level editor (opens .level and .level.json files)
    registry.register_provider(Arc::new(LevelEditorBuiltinProvider));

    // Every enabled plugin's editors, submitted from its crate
    // (`plugin_manager::PluginEditorProvider`), and other linked providers.
    registry.register_linked();

    tracing::info!("Built-in editor registration complete");
}
