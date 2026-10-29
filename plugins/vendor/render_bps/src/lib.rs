//! # Render Graph Editor
//!
//! Builds Helio render pipelines visually on the shared Graphy canvas
//! (`graphy-gpui`): nodes are render stages and resources, pins are textures,
//! depth, buffers and host inputs, and the document is a `.rendergraph` asset that
//! `helio_graph_asset` compiles into a `RenderGraph`.
//!
//! The Graphy canvas owns the one editable document ([`document`]); this crate is
//! the render graph domain on top of it: the node catalogue and dynamic Helio
//! pins ([`model`], [`profile`]), the `.rendergraph` boundary ([`document`]), the
//! editor chrome ([`editor`], [`view`]), the Sections and Buffers panels and the
//! pipeline preview.
//!
//! A project's `.rendergraph` assets are offered as render pipelines in the
//! level editor's World Settings, next to Helio's built-in ones.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;
use plugin_editor_api::*;
use ui::dock::PanelView;

mod buffers_panel;
mod collapse;
pub mod document;
pub mod editor;
pub mod model;
mod preview;
pub mod profile;
mod theme;
mod sections_panel;
mod view;

#[cfg(test)]
mod tests;

/// Extension of a render graph asset.
const EXTENSION: &str = helio_graph_asset::EXTENSION;
const FILE_TYPE: &str = "rendergraph";
const EDITOR: &str = "render-graph-editor";

/// Starts learning the pins of Helio's nodes from their passes, once, in the
/// background, on the editor's own device when there is one (so no second device
/// is made), so they are ready by the time a graph is opened.
fn start_pin_probe(window: Option<&Window>) {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        use engine_backend::subsystems::render::helio_renderer::pipelines;
        if let Some((device, queue)) = window.and_then(Window::gpu_device_and_queue) {
            pipelines::offer_probe_device(device, queue);
        }
        pipelines::start_graph_pin_probe();
    });
}

/// Opens an editor on the render graph at `file_path`.
pub(crate) fn open_editor(
    file_path: PathBuf,
    window: &mut Window,
    cx: &mut App,
) -> Entity<editor::RenderGraphEditor> {
    start_pin_probe(Some(&*window));
    cx.new(|cx| editor::RenderGraphEditor::open(file_path, window, cx))
}

pub(crate) fn file_type() -> FileTypeDefinition {
    // A new graph is a marker the editor expands when it opens the file: the
    // default deferred pipeline, wired from the pins its passes declare. Those
    // are known only once the pin probe has run, which a file written at startup
    // cannot wait for.
    FileTypeDefinition {
        id: FileTypeId::new(FILE_TYPE),
        extension: EXTENSION.to_string(),
        display_name: "Render Graph".to_string(),
        icon: ui::IconName::Component,
        color: gpui::rgb(0x2B6CB0).into(),
        structure: FileStructure::Standalone,
        default_content: serde_json::json!({
            "format_version": helio_graph_asset::FORMAT_VERSION,
            "template": "deferred",
        }),
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
        Self {
            editors: Default::default(),
            next_id: Default::default(),
        }
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
        start_pin_probe(None);
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
        let panel: Arc<dyn PanelView> = Arc::new(open_editor(file_path, window, cx));
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
