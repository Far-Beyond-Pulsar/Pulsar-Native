//! # Blueprint Editor Plugin
//!
//! This plugin provides visual scripting capabilities through the Blueprint Editor.
//! It supports .class files (folder-based) that contain node graphs for visual programming.
//!
//! ## File Types
//!
//! - **Blueprint Class** (.class folder)
//!   - Contains `graph_save.json` with the node graph
//!   - Contains `events/` folder for event handlers
//!   - Appears as a single file in the file drawer
//!
//! ## Editors
//!
//! - **Blueprint Editor**: Visual node-based scripting interface

use plugin_editor_api::*;
use serde_json::json;
use std::path::PathBuf;
use gpui::*;
use ui::dock::{Panel, PanelEvent};

// Blueprint Editor modules
mod blueprint_types;
mod events;
mod node_graph;
mod toolbar;
mod properties;
mod variables;
mod file_drawer;
mod node_creation_menu;
mod macros;
mod minimap;
mod hoverable_tooltip;
mod node_palette;
mod node_library;

// Panel module (main editor implementation)
pub mod panel;

// Re-export main types
pub use blueprint_types::*;
pub use panel::BlueprintEditorPanel;
pub use events::*;

/// The Blueprint Editor Plugin
pub struct BlueprintEditorPlugin;

impl Default for BlueprintEditorPlugin {
    fn default() -> Self {
        Self
    }
}

impl EditorPlugin for BlueprintEditorPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.pulsar.blueprint-editor"),
            name: "Blueprint Editor".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Visual scripting editor for creating blueprint classes".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![folder_file_type(
            "blueprint-class",
            "class",
            "Blueprint Class",
            FileIcon::Component,
            "graph_save.json",
            vec![
                PathTemplate::Folder {
                    path: "events".into(),
                },
            ],
            json!({
                "graph": {
                    "nodes": [],
                    "connections": [],
                    "comments": [],
                    "metadata": {
                        "version": "0.1.0"
                    }
                }
            }),
        )]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("blueprint-editor"),
            display_name: "Blueprint Editor".into(),
            supported_file_types: vec![FileTypeId::new("blueprint-class")],
        }]
    }

    fn create_editor(
        &self,
        editor_id: EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Box<dyn EditorInstance>, PluginError> {
        if editor_id.as_str() == "blueprint-editor" {
            let panel = panel::BlueprintEditorPanel::new_with_path(file_path.clone(), window, cx)
                .map_err(|e| PluginError::FileLoadError {
                    path: file_path.clone(),
                    message: e.to_string(),
                })?;

            Ok(Box::new(panel))
        } else {
            Err(PluginError::EditorNotFound { editor_id })
        }
    }

    fn on_load(&mut self) {
        log::info!("Blueprint Editor Plugin loaded");
    }

    fn on_unload(&mut self) {
        log::info!("Blueprint Editor Plugin unloaded");
    }
}

// Export the plugin using the provided macro
export_plugin!(BlueprintEditorPlugin);
