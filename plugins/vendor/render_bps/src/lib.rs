//! # Render Graph Editor
//!
//! Builds Helio render pipelines visually. It is the Blueprint graph editor
//! with the Blueprint-specific parts replaced (see
//! `blueprint_editor_plugin::profile`): nodes are render stages and resources,
//! pins are textures, depth, buffers and host inputs, and the document is a
//! `.rendergraph` asset that `helio_graph_asset` compiles into a `RenderGraph`.
//!
//! A project's `.rendergraph` assets are offered as render pipelines in the
//! level editor's World Settings, next to Helio's built-in ones.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use blueprint_editor_plugin::profile::{install_profile, EditorProfile, ParamField, PinStyle};
use blueprint_editor_plugin::{BlueprintEditorPanel, BlueprintGraph};
use gpui::*;
use plugin_editor_api::*;
use ui::dock::PanelView;

mod model;

use model::{LiveNode, LiveWire};

/// Extension of a render graph asset.
const EXTENSION: &str = helio_graph_asset::EXTENSION;
const FILE_TYPE: &str = "rendergraph";
const EDITOR: &str = "render-graph-editor";

/// Installs the profile once, before any editor opens a render graph. Safe to
/// call from every entry point.
pub(crate) fn ensure_profile() {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    INSTALL.call_once(|| {
        if let Err(error) = install_profile(Arc::new(RenderProfile)) {
            log::error!("Render Graph Editor: {error}");
        }
    });
}

/// What replaces the Blueprint specifics in the shared graph editor.
struct RenderProfile;

impl EditorProfile for RenderProfile {
    fn id(&self) -> &str {
        "render-graph"
    }

    fn extensions(&self) -> Vec<String> {
        vec![EXTENSION.to_string()]
    }

    fn catalog(&self) -> blueprint_editor_plugin::NodeDefinitions {
        model::catalog(model::registry())
    }

    fn decode(&self, text: &str) -> Result<blueprint_graph::BlueprintAsset, String> {
        let asset = helio_graph_asset::GraphAsset::from_json(text)?;
        model::to_blueprint_asset(&asset, model::registry())
    }

    fn encode(&self, asset: &blueprint_graph::BlueprintAsset) -> Result<String, String> {
        Ok(model::from_blueprint_asset(asset, model::registry()).to_json())
    }

    fn pin_style(&self, type_name: &str) -> PinStyle {
        model::pin_style(type_name)
    }

    fn parameters(&self, definition_id: &str) -> Vec<ParamField> {
        model::parameter_fields(definition_id, model::registry())
    }

    fn parameter_changed(&self, graph: &mut BlueprintGraph, node_id: &str) {
        model::refresh_pins(graph, node_id, model::registry());
    }

    fn validate(&self, graph: &BlueprintGraph) -> Vec<String> {
        let nodes: Vec<LiveNode> = graph
            .nodes
            .iter()
            .map(|node| LiveNode {
                id: node.id.clone(),
                definition_id: node.definition_id.clone(),
                position: [node.position.x, node.position.y],
                properties: node.properties.clone(),
            })
            .collect();
        let wires: Vec<LiveWire> = graph
            .connections
            .iter()
            .map(|c| LiveWire {
                source_node: c.source_node.clone(),
                source_pin: c.source_pin.clone(),
                target_node: c.target_node.clone(),
                target_pin: c.target_pin.clone(),
            })
            .collect();
        let registry = model::registry();
        let asset = model::asset_from_live("", &nodes, &wires, registry);
        model::problems(&asset, registry)
    }
}

pub(crate) fn file_type() -> FileTypeDefinition {
    let template = helio_default_graphs::nodes::default_graph_asset();
    FileTypeDefinition {
        id: FileTypeId::new(FILE_TYPE),
        extension: EXTENSION.to_string(),
        display_name: "Render Graph".to_string(),
        icon: ui::IconName::Component,
        color: gpui::rgb(0x2B6CB0).into(),
        structure: FileStructure::Standalone,
        default_content: serde_json::from_str(&template.to_json())
            .unwrap_or(serde_json::Value::Null),
        creation_directory: None,
        categories: vec!["Rendering".to_string()],
    }
}

/// The Render Graph editor plugin.
pub struct RenderBpsPlugin {
    /// Editors stay alive as long as the plugin; the host holds raw handles.
    editors: std::sync::Mutex<HashMap<usize, Arc<dyn PanelView>>>,
    next_id: std::sync::atomic::AtomicUsize,
}

impl Default for RenderBpsPlugin {
    fn default() -> Self {
        Self { editors: Default::default(), next_id: Default::default() }
    }
}

impl EditorPlugin for RenderBpsPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.pulsar.render-bps"),
            name: "Render Graph Editor".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Visual editor for Helio render pipelines".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![file_type()]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new(EDITOR),
            display_name: "Render Graph Editor".into(),
            supported_file_types: vec![FileTypeId::new(FILE_TYPE)],
        }]
    }

    fn on_load(&mut self) {
        ensure_profile();
        log::info!("Render Graph Editor plugin loaded");
    }
}

impl RenderBpsPlugin {
    fn create_editor(
        &'static self,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        ensure_profile();
        let panel = cx.new(|cx| {
            match BlueprintEditorPanel::new_with_path(file_path.clone(), window, cx) {
                Ok(panel) => panel,
                Err(error) => {
                    tracing::error!("Failed to open render graph {file_path:?}: {error}");
                    BlueprintEditorPanel::new_with_load_error(
                        file_path.clone(),
                        error.to_string(),
                        window,
                        cx,
                    )
                }
            }
        });
        let panel: Arc<dyn PanelView> = Arc::new(panel);
        let id = self.next_id.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.editors.lock().unwrap().insert(id, panel.clone());
        Ok(panel)
    }
}

impl EditorPluginEditor for RenderBpsPlugin {
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
        registry.register_fn(EditorId::new(EDITOR), |file_path, window, cx| {
            self.create_editor(file_path, window, cx)
        });
    }
}

impl EditorPluginStatusbar for RenderBpsPlugin {}
impl EditorPluginAi for RenderBpsPlugin {}
impl EditorPluginComponents for RenderBpsPlugin {
    fn component_definitions(&self) -> Vec<ComponentDefinition> {
        Vec::new()
    }
}
impl EditorPluginSubsystems for RenderBpsPlugin {
    fn subsystems(&self) -> Vec<Box<dyn Subsystem>> {
        Vec::new()
    }
}
impl EditorPluginScripting for RenderBpsPlugin {}

#[cfg(feature = "builtin")]
mod builtin_provider;
#[cfg(feature = "builtin")]
pub use builtin_provider::RenderBpsBuiltinProvider;

// A statically linked build registers through its provider; only the plugin
// DLL exports the loader symbols.
#[cfg(not(feature = "builtin"))]
export_plugin!(RenderBpsPlugin);
