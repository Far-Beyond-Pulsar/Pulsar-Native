//! Central registration for all built-in editors.
//!
//! This module provides a single function to register all built-in editors
//! with the plugin manager's registries.

use gpui::AppContext;
use gpui::{App, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext};
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::PanelView;

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

    tracing::info!("Built-in editor registration complete");
}
