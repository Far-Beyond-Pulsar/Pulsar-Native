//! Central registration for all built-in editors.
//!
//! This module provides a single function to register all built-in editors
//! with the plugin manager's registries.

use engine_backend::services::rust_analyzer_manager::RustAnalyzerManager;
use gpui::AppContext;
use gpui::{App, Entity, Global, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dock::PanelView;

#[derive(Clone)]
pub struct SharedScriptEditorAnalyzer(pub Entity<RustAnalyzerManager>);

impl Global for SharedScriptEditorAnalyzer {}

pub fn set_shared_script_editor_analyzer(cx: &mut App, analyzer: Entity<RustAnalyzerManager>) {
    if cx.has_global::<SharedScriptEditorAnalyzer>() {
        cx.global_mut::<SharedScriptEditorAnalyzer>().0 = analyzer;
    } else {
        cx.set_global(SharedScriptEditorAnalyzer(analyzer));
    }
}

fn get_shared_script_editor_analyzer(cx: &App) -> Option<Entity<RustAnalyzerManager>> {
    cx.try_global::<SharedScriptEditorAnalyzer>()
        .map(|shared| shared.0.clone())
}

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
                "format_version": 1,
                "main_graph": {
                    "nodes": {},
                    "connections": [],
                    "metadata": {
                        "name": "EventGraph",
                        "description": "",
                        "version": "1.0.0",
                        "created_at": "2024-01-01T00:00:00+00:00",
                        "modified_at": "2024-01-01T00:00:00+00:00"
                    },
                    "comments": []
                },
                "local_macros": [],
                "variables": [],
                "blueprint_metadata": {
                    "blueprint_type": "Generic",
                    "parent_class": null,
                    "description": "",
                    "category": "Uncategorized",
                    "tags": []
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

    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        blueprint_editor_plugin::BlueprintEditorPlugin::default().ai_tools()
    }

    fn capabilities_for_file(&self, file_path: &Path) -> Vec<String> {
        blueprint_editor_plugin::BlueprintEditorPlugin::default().capabilities_for_file(file_path)
    }

    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        tool_args: JsonValue,
    ) -> Result<JsonValue, PluginError> {
        blueprint_editor_plugin::execute_compiled_tool(file_path, tool_name, tool_args)
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

        // Keep plugin AI tools aligned with the currently opened blueprint panel state.
        let graph_snapshot = panel.read(cx).graph.clone();
        blueprint_editor_plugin::upsert_ai_session(file_path.clone(), graph_snapshot);

        Ok(Arc::new(panel))
    }
}

// ---------------------------------------------------------------------------
// Script Editor — built-in provider (no DLL boundary)
// ---------------------------------------------------------------------------

/// Wraps the script_editor_plugin crate as a built-in editor provider.
pub struct ScriptEditorBuiltinProvider;

impl BuiltinEditorProvider for ScriptEditorBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.script-editor"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        use serde_json::json;

        vec![
            FileTypeDefinition {
                id: FileTypeId::new("rust_script"),
                extension: "rs".to_string(),
                display_name: "Rust".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0xFF5722).into(),
                structure: FileStructure::Standalone,
                default_content: json!("// New Rust script\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("javascript"),
                extension: "js".to_string(),
                display_name: "JavaScript".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0xF7DF1E).into(),
                structure: FileStructure::Standalone,
                default_content: json!("// New JavaScript file\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("typescript"),
                extension: "ts".to_string(),
                display_name: "TypeScript".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0x3178C6).into(),
                structure: FileStructure::Standalone,
                default_content: json!("// New TypeScript file\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("python"),
                extension: "py".to_string(),
                display_name: "Python Script".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0x3776AB).into(),
                structure: FileStructure::Standalone,
                default_content: json!("# New Python script\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("lua"),
                extension: "lua".to_string(),
                display_name: "Lua Script".to_string(),
                icon: ui::IconName::Code,
                color: gpui::rgb(0x2196F3).into(),
                structure: FileStructure::Standalone,
                default_content: json!("-- New Lua script\n"),
                categories: vec!["Scripts".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("toml"),
                extension: "toml".to_string(),
                display_name: "TOML Configuration".to_string(),
                icon: ui::IconName::Page,
                color: gpui::rgb(0x9E9E9E).into(),
                structure: FileStructure::Standalone,
                default_content: json!("# TOML configuration file\n"),
                categories: vec!["Data".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("markdown"),
                extension: "md".to_string(),
                display_name: "Markdown Document".to_string(),
                icon: ui::IconName::Page,
                color: gpui::rgb(0xFF5722).into(),
                structure: FileStructure::Standalone,
                default_content: json!("# New Document\n"),
                categories: vec!["Documents".to_string()],
            },
        ]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("script-editor"),
            display_name: "Script Editor".into(),
            supported_file_types: vec![
                FileTypeId::new("rust_script"),
                FileTypeId::new("javascript"),
                FileTypeId::new("typescript"),
                FileTypeId::new("python"),
                FileTypeId::new("lua"),
                FileTypeId::new("toml"),
                FileTypeId::new("markdown"),
            ],
        }]
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "script-editor"
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let shared_analyzer = get_shared_script_editor_analyzer(cx);
        let panel = cx.new(|cx| script_editor_plugin::ScriptEditorPanel::new(window, cx));
        panel.update(cx, |editor, ecx| {
            if let Some(analyzer) = shared_analyzer.clone() {
                editor.set_rust_analyzer(analyzer, ecx);
            } else {
                tracing::warn!(
                    "Shared Rust analyzer not available; Script Editor LSP will be limited"
                );
            }
            editor.open_file(file_path.clone(), window, ecx);
        });

        Ok(Arc::new(panel))
    }
}

// ---------------------------------------------------------------------------
// Matter Editor — built-in provider (Pulsar Image Format / texture painter)
// ---------------------------------------------------------------------------

/// Opens `.pif` files in the Matter texture-painting editor.
pub struct MatterEditorBuiltinProvider;

impl BuiltinEditorProvider for MatterEditorBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.matter-editor"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        // PIF assets are directory-backed bundles named `*.pif`.
        // Register as FolderBased so `Path::is_dir()` resolution still maps them
        // to the Matter editor during open-path lookup.
        vec![FileTypeDefinition {
            id: FileTypeId::new("pif"),
            extension: "pif".to_string(),
            display_name: "Pulsar Image Format".to_string(),
            icon: ui::IconName::EditPencil,
            color: gpui::rgb(0xE91E63).into(),
            structure: FileStructure::FolderBased {
                marker_file: "manifest.json".to_string(),
                template_structure: vec![PathTemplate::Folder {
                    path: ".raster".into(),
                }],
            },
            default_content: serde_json::json!({
                "pif_version": "1.0",
                "canvas": {
                    "width": 1024,
                    "height": 1024,
                    "color_space": "sRGB"
                },
                "layers": [
                    {
                        "type": "raster",
                        "id": "background",
                        "name": "Background",
                        "visible": true,
                        "opacity": 1.0,
                        "blend_mode": "normal",
                        "tile_size": 256,
                        "tiles": {}
                    }
                ]
            }),
            categories: vec!["Textures".to_string()],
        }]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("matter-editor"),
            display_name: "Matter Editor".into(),
            supported_file_types: vec![FileTypeId::new("pif")],
        }]
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "matter-editor"
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        use gpui::Rgba;
        use plugin_matter::brush_engine::{BrushDropdownItem, BrushRegistry};
        use plugin_matter::state::Document;
        use ui::{color_picker::ColorPickerState, dropdown::DropdownState, IndexPath};

        let document = if file_path.exists() {
            Document::open(file_path.clone()).map_err(|e| PluginError::Other {
                message: format!("Failed to open PIF file {:?}: {}", file_path, e),
            })?
        } else {
            Document::new(1024, 1024).map_err(|e| PluginError::Other {
                message: format!("Failed to create new PIF document: {}", e),
            })?
        };

        let fg = cx.new(|cx| {
            ColorPickerState::new(window, cx).default_value(
                Rgba {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                }
                .into(),
            )
        });
        let bg = cx.new(|cx| {
            ColorPickerState::new(window, cx).default_value(
                Rgba {
                    r: 1.0,
                    g: 1.0,
                    b: 1.0,
                    a: 1.0,
                }
                .into(),
            )
        });

        let brushes_dir = std::env::current_dir().unwrap_or_default().join("brushes");
        let brush_registry = Arc::new(BrushRegistry::load_from_dir(&brushes_dir));
        let items: Vec<BrushDropdownItem> = brush_registry.dropdown_items();

        let active_brush_id = document.tool_state.active_brush_id.clone();
        let initial = items
            .iter()
            .position(|item| item.id == active_brush_id)
            .map(|idx| IndexPath::default().row(idx))
            .or_else(|| (!items.is_empty()).then(|| IndexPath::default().row(0)));

        let brush_dropdown = cx.new(|cx| DropdownState::new(items, initial, window, cx));
        let panel = cx.new(|cx| {
            plugin_matter::MatterEditorPanel::new(
                document,
                fg,
                bg,
                brush_dropdown,
                brush_registry.clone(),
                cx,
            )
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

    // Blueprint editor (compiled-in, no DLL boundary)
    registry.register_provider(Arc::new(BlueprintEditorBuiltinProvider));

    // Script editor (compiled-in, no DLL boundary)
    registry.register_provider(Arc::new(ScriptEditorBuiltinProvider));

    // Level editor (opens .level and .level.json files)
    registry.register_provider(Arc::new(LevelEditorBuiltinProvider));

    // Matter editor (opens .pif Pulsar Image Format texture files)
    registry.register_provider(Arc::new(MatterEditorBuiltinProvider));

    tracing::info!("Built-in editor registration complete");
}
