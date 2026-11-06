//! Materials (`pulsar.materials`): the editor plugin.
//!
//! The plugin's identity ([`MaterialsPlugin`]) and its editor: the material
//! graph editor for `.material` assets (`materials_graph_editor`), registered
//! as a linked editor provider ([`MaterialGraphEditorProvider`]). The
//! components and every runtime registration are the component crate's
//! (`materials_component`), which this crate links: the editor links this
//! crate (through `installed_plugins`, as an installed plugin's editor
//! module).

use plugin_editor_api::*;

/// Links the component crate, so its registrations come with this crate.
pub use materials_component as component;

/// The plugin's identity.
#[derive(Default)]
pub struct MaterialsPlugin;

impl MaterialsPlugin {
    /// The plugin id.
    pub const ID: &'static str = "pulsar.materials";
}

impl EditorPlugin for MaterialsPlugin {
    fn metadata(&self) -> PluginMetadata {
        // The plugin's identity is its descriptor's (pulsar-plugin.toml).
        let descriptor = materials_component::plugin_descriptor();
        PluginMetadata {
            id: PluginId::new(descriptor.id),
            name: descriptor.name,
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: descriptor.description,
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        materials_graph_editor::file_types()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        materials_graph_editor::editors()
    }
}

/// The material graph editor as a linked editor provider: opens `.material`
/// assets, offers the graph's AI tools, and does the graph editor's startup
/// work (node definitions, thumbnails, the import companion) when the host
/// registers it.
pub struct MaterialGraphEditorProvider;

impl MaterialGraphEditorProvider {
    fn create() -> std::sync::Arc<dyn plugin_manager::BuiltinEditorProvider> {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            materials_graph_editor::initialize();
            // Plugins tell the import pipeline what extra work they can do
            // for an import; the pipeline knows nothing about materials.
            asset_import::register_companion(materials_graph_editor::material_import_companion());
        });
        std::sync::Arc::new(MaterialGraphEditorProvider)
    }
}

plugin_manager::inventory::submit! {
    plugin_manager::PluginEditorProvider {
        origin: pulsar_plugins::origin!(),
        create: MaterialGraphEditorProvider::create,
    }
}

impl plugin_manager::BuiltinEditorProvider for MaterialGraphEditorProvider {
    fn provider_id(&self) -> &str {
        MaterialsPlugin::ID
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        materials_graph_editor::file_types()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        materials_graph_editor::editors()
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == materials_graph_editor::EDITOR_ID
    }

    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        materials_graph_editor::ai_tools()
    }

    fn capabilities_for_file(&self, file_path: &std::path::Path) -> Vec<String> {
        materials_graph_editor::capabilities_for_file(file_path)
    }

    fn execute_ai_tool(
        &self,
        file_path: &std::path::Path,
        tool_name: &str,
        tool_args: serde_json::Value,
    ) -> Result<serde_json::Value, PluginError> {
        materials_graph_editor::execute_compiled_tool(file_path, tool_name, tool_args)
    }

    fn create_editor(
        &self,
        file_path: std::path::PathBuf,
        editor_context: &plugin_manager::EditorContext,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Result<std::sync::Arc<dyn ui::dock::PanelView>, PluginError> {
        use gpui::AppContext as _;
        use materials_graph_editor::ShaderEditorPanel;
        let panel = cx.new(|cx| {
            ShaderEditorPanel::new_with_path(file_path.clone(), window, cx).unwrap_or_else(|e| {
                tracing::error!("Failed to load material {:?}: {}", file_path, e);
                ShaderEditorPanel::new(window, cx)
            })
        });
        let undo = editor_context.services.undo.clone();
        panel.update(cx, |panel, cx| panel.set_undo_service(undo, cx));
        // Keep the AI tools aligned with the opened material's graph.
        let graph = panel.read(cx).graph_snapshot(cx);
        materials_graph_editor::upsert_ai_session(file_path, graph);
        Ok(std::sync::Arc::new(panel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_the_plugin_id() {
        assert_eq!(MaterialsPlugin.metadata().id.as_str(), "pulsar.materials");
    }
}
