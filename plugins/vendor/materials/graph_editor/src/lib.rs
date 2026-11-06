#![recursion_limit = "512"]

//! # Material graph editor (`pulsar.materials`)
//!
//! Visual shader graph editor for creating materials through node-based programming.
//! The graph is drawn, laid out, hit-tested and edited by the shared Graphy canvas
//! (`graphy-gpui`); this crate supplies what is specific to shaders.
//!
//! ## Architecture
//!
//! - **editor**: the editor panel (canvas, history, dock workspace, toolbar)
//! - **profile** / **types**: the Graphy profile: node catalogue, WGSL connection
//!   rules, node and pin look, preview rows
//! - **previews**: live texture previews filling those rows
//! - **definitions**: shader node definitions from PSGC
//! - **features**: compilation (PSGC), material preview, shader models, import
//! - **ui_components**: the Details panel
//! - **io**: the `.material` asset and its save/load

use gpui::*;
use plugin_editor_api::*;
use serde_json::json;
use std::path::PathBuf;

// Module declarations
mod actions;
mod ai_tools;
mod definitions;
mod editor;
mod features;
mod io;
mod previews;
mod profile;
mod theme;
mod types;
mod ui_components;

// Re-export main types for plugin API compatibility
pub use actions::*;
pub use definitions::*;
pub use editor::panel::ShaderEditorPanel;

pub fn upsert_ai_session(file_path: PathBuf, graph: graphy::GraphDescription) {
    ai_tools::upsert_session(file_path, graph);
}

pub fn execute_compiled_tool(
    file_path: &std::path::Path,
    tool_name: &str,
    tool_args: serde_json::Value,
) -> Result<serde_json::Value, PluginError> {
    ai_tools::execute_compiled_tool(file_path, tool_name, tool_args)
}

/// Register the renderer of the `.material` assets' thumbnails (part of
/// [`initialize`]).
pub fn register_material_thumbnail_renderer() {
    engine_fs::thumbnails::register_thumbnail_renderer(
        "material",
        crate::features::preview::thumbnail::render_material_thumbnail,
    );
}

/// The import companion that offers to build materials from the images found
/// with an imported model. The materials plugin registers it with the import
/// pipeline at startup (the pipeline itself knows nothing about materials).
pub use features::import_companion::material_import_companion;

/// The material graph editor's id.
pub const EDITOR_ID: &str = "shader-editor";

/// The `.material` asset (a folder holding the graph).
pub fn file_types() -> Vec<FileTypeDefinition> {
    vec![FileTypeDefinition {
        id: FileTypeId::new("material"),
        extension: "material".to_string(),
        display_name: "Material".to_string(),
        icon: ui::IconName::Component,
        color: gpui::rgb(0x4CAF50).into(),
        structure: FileStructure::FolderBased {
            marker_file: "shader_graph_save.json".to_string(),
            template_structure: vec![],
        },
        default_content: json!({
            "format_version": 1,
            "main_graph": {
                "nodes": {},
                "connections": [],
                "metadata": {
                    "name": "MainMaterial",
                    "description": "",
                    "version": "1.0.0",
                    "created_at": "2024-01-01T00:00:00+00:00",
                    "modified_at": "2024-01-01T00:00:00+00:00"
                },
                "comments": []
            },
            "shader_metadata": {
                "shader_model": "Standard_unlit",
                "stage": "Fragment",
                "description": "",
                "category": "Uncategorized"
            }
        }),
        creation_directory: None,

        categories: vec!["Shaders".to_string()],
    }]
}

/// The material graph editor, for `.material` assets.
pub fn editors() -> Vec<EditorMetadata> {
    vec![EditorMetadata {
        id: EditorId::new(EDITOR_ID),
        display_name: "Material Graph Editor".into(),
        supported_file_types: vec![FileTypeId::new("material")],
    }]
}

/// The editor's AI tools.
pub fn ai_tools() -> Vec<AiToolDefinition> {
    ai_tools::ai_tools()
}

/// The AI capabilities a material file offers.
pub fn capabilities_for_file(file_path: &std::path::Path) -> Vec<String> {
    ai_tools::capabilities_for_file(file_path)
}

/// Startup work the editor does once (the host has no plugin `on_load` for
/// a linked editor): the node definitions, the material thumbnail renderer.
pub fn initialize() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        crate::features::initialize_features();
        register_material_thumbnail_renderer();
    });
}
