//! Central registration for all built-in editors.
//!
//! This module provides a single function to register all built-in editors
//! with the plugin manager's registries.

use gpui::AppContext;
use gpui::{App, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext};
use std::{fs, path::{Path, PathBuf}};
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
        vec![
            AiToolDefinition::new(
                "level_editor_validate_level_file",
                "Validate that a .level/.level.json file has required Level Editor structure.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                }),
            )
            .with_category("validation"),
            AiToolDefinition::new(
                "level_editor_count_objects",
                "Return a count of objects in a level file.",
                serde_json::json!({
                    "type": "object",
                    "properties": {},
                }),
            )
            .with_category("analysis"),
        ]
    }

    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        let is_level_file = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(".level") || name.ends_with(".level.json"))
            .unwrap_or(false);

        if is_level_file {
            vec![
                "level_editor_validate_level_file".to_string(),
                "level_editor_count_objects".to_string(),
            ]
        } else {
            Vec::new()
        }
    }

    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        _tool_args: JsonValue,
    ) -> Result<JsonValue, PluginError> {
        let content = fs::read_to_string(file_path).map_err(|err| PluginError::FileLoadError {
            path: file_path.to_path_buf(),
            message: err.to_string(),
        })?;

        let parsed: serde_json::Value = serde_json::from_str(&content).map_err(|err| {
            PluginError::InvalidFormat {
                expected: "Pulsar level JSON".to_string(),
                message: err.to_string(),
            }
        })?;

        match tool_name {
            "level_editor_validate_level_file" => {
                let mut issues = Vec::new();
                if !parsed.is_object() {
                    issues.push("Root must be a JSON object".to_string());
                }
                if parsed.get("version").is_none() {
                    issues.push("Missing required field: version".to_string());
                }
                if !parsed
                    .get("objects")
                    .map(|objects| objects.is_array())
                    .unwrap_or(false)
                {
                    issues.push("Missing or invalid required field: objects (array)".to_string());
                }

                Ok(serde_json::json!({
                    "ok": issues.is_empty(),
                    "issue_count": issues.len(),
                    "issues": issues,
                }))
            }
            "level_editor_count_objects" => {
                let object_count = parsed
                    .get("objects")
                    .and_then(|objects| objects.as_array())
                    .map(|objects| objects.len())
                    .unwrap_or(0);

                Ok(serde_json::json!({
                    "ok": true,
                    "object_count": object_count,
                }))
            }
            _ => Err(PluginError::Other {
                message: format!("Unknown Level Editor AI tool: {tool_name}"),
            }),
        }
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let panel = cx.new(|cx| {
            match ui_level_editor::LevelEditorPanel::new_with_path(file_path.clone(), window, cx) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("Failed to load level {:?}: {}", file_path, e);
                    ui_level_editor::LevelEditorPanel::new(window, cx)
                }
            }
        });
        Ok(Arc::new(panel))
    }
}

// ---------------------------------------------------------------------------
// Blueprint Editor — built-in provider (no DLL boundary)
// ---------------------------------------------------------------------------

/// Wraps the blueprint_editor_plugin crate as a built-in editor provider.
/// All types, vtables, and drop glue live in the same binary — no FFI needed.
pub struct BlueprintEditorBuiltinProvider;

impl BuiltinEditorProvider for BlueprintEditorBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.blueprint-editor"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        use serde_json::json;

        vec![FileTypeDefinition {
            id: FileTypeId::new("class"),
            extension: "class".to_string(),
            display_name: "Blueprint Class".to_string(),
            icon: ui::IconName::Component,
            color: gpui::rgb(0x9C27B0).into(),
            structure: FileStructure::FolderBased {
                marker_file: "graph_save.json".to_string(),
                template_structure: vec![PathTemplate::Folder {
                    path: "events".into(),
                }],
            },
            default_content: json!({
                "graph": {
                    "nodes": [],
                    "connections": [],
                    "comments": [],
                    "metadata": {
                        "version": "0.1.0"
                    }
                }
            }),
            categories: vec!["Blueprints".to_string()],
        }]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("blueprint-editor"),
            display_name: "Blueprint Editor".into(),
            supported_file_types: vec![FileTypeId::new("class")],
        }]
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "blueprint-editor"
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let panel =
            cx.new(|cx| {
                match blueprint_editor_plugin::BlueprintEditorPanel::new_with_path(
                    file_path.clone(),
                    window,
                    cx,
                ) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("Failed to create blueprint panel: {}", e);
                        blueprint_editor_plugin::BlueprintEditorPanel::new(window, cx)
                    }
                }
            });

        Ok(Arc::new(panel))
    }
}

// ---------------------------------------------------------------------------
// File Manager Tools — built-in provider (AI tools only)
// ---------------------------------------------------------------------------

/// Exposes file-manager AI tools via the built-in provider path.
pub struct FileManagerBuiltinProvider;

impl BuiltinEditorProvider for FileManagerBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.file-manager"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        Vec::new()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        Vec::new()
    }

    fn can_handle(&self, _editor_id: &EditorId) -> bool {
        false
    }

    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        ui_file_manager::ai_tools::ai_tools()
    }

    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        ui_file_manager::ai_tools::capabilities_for_file(file_path)
    }

    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        tool_args: JsonValue,
    ) -> Result<JsonValue, PluginError> {
        ui_file_manager::ai_tools::execute_ai_tool(file_path, tool_name, tool_args)
    }

    fn create_editor(
        &self,
        _file_path: PathBuf,
        _editor_context: &EditorContext,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        Err(PluginError::EditorNotFound {
            editor_id: EditorId::new("file-manager-tools"),
        })
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

    // Blueprint editor (compiled-in, no DLL boundary)
    registry.register_provider(Arc::new(BlueprintEditorBuiltinProvider));

    // Level editor (opens .level and .level.json files)
    registry.register_provider(Arc::new(LevelEditorBuiltinProvider));

    // File manager AI tools provider (no editor surface)
    registry.register_provider(Arc::new(FileManagerBuiltinProvider));

    tracing::info!("Built-in editor registration complete");
}
