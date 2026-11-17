use gpui::*;
use plugin_editor_api::*;
use rust_i18n::t;
use serde_json::json;
use std::{path::PathBuf, sync::Arc};
use ui::dock::PanelView;

rust_i18n::i18n!("locales", fallback = "en");

mod editor;
pub use editor::panel::AssetViewerPanel;

#[derive(Default)]
pub struct AssetViewerPlugin;

impl EditorPlugin for AssetViewerPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.pulsar.asset-viewer"),
            name: t!("AssetViewer.DisplayName").to_string().into(),
            version: "0.1.0".into(),
            author: t!("AssetViewer.Team").to_string().into(),
            description: "View FBX 3D models and PNG images".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![
            FileTypeDefinition {
                id: FileTypeId::new("fbx"),
                extension: "fbx".to_string(),
                display_name: t!("AssetViewer.FBXModel").to_string(),
                icon: ui::IconName::Cube,
                color: gpui::rgb(0x00BCD4).into(),
                structure: FileStructure::Standalone,
                default_content: json!(""),
                creation_directory: None,
                cook: plugin_editor_api::file_types::copy_file_for_cook,

                categories: vec!["3D Models".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("mesh"),
                extension: "mesh".to_string(),
                display_name: t!("AssetViewer.NativeMesh").to_string(),
                icon: ui::IconName::Cube,
                color: gpui::rgb(0x00BCD4).into(),
                structure: FileStructure::Standalone,
                default_content: json!(""),
                creation_directory: None,
                cook: mesh_asset_cook,

                categories: vec!["3D Models".to_string()],
            },
            FileTypeDefinition {
                id: FileTypeId::new("png"),
                extension: "png".to_string(),
                display_name: t!("AssetViewer.PNGImage").to_string(),
                icon: ui::IconName::Image,
                color: gpui::rgb(0xE91E63).into(),
                structure: FileStructure::Standalone,
                default_content: json!(""),
                creation_directory: None,
                cook: plugin_editor_api::file_types::copy_file_for_cook,

                categories: vec![t!("AssetViewer.Images").to_string()],
            },
        ]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("asset-viewer"),
            display_name: "Asset Viewer".into(),
            supported_file_types: vec![
                FileTypeId::new("fbx"),
                FileTypeId::new("mesh"),
                FileTypeId::new("png"),
            ],
        }]
    }
}

/// Native meshes own their default material assignments in the PMSH metadata.
fn mesh_asset_cook(
    context: &plugin_editor_api::CookContext<'_>,
) -> Result<plugin_editor_api::file_types::CookOutput, String> {
    let cooked = plugin_editor_api::file_types::copy_file_for_cook(context)?;
    let bytes = cooked
        .files
        .first()
        .ok_or_else(|| format!("{} emitted no mesh", context.source_path.display()))?
        .bytes
        .as_slice();
    let (mesh, _) = meshes_component::mesh_cache::decode_asset(&bytes).ok_or_else(|| {
        format!(
            "{} is not a valid native mesh",
            context.source_path.display()
        )
    })?;
    let dependencies = mesh
        .material_slots
        .into_iter()
        .map(|slot| slot.material_asset.trim().to_owned())
        .filter(|path| !path.is_empty())
        .collect();
    Ok(plugin_editor_api::file_types::CookOutput::new(
        cooked.files,
        dependencies,
    ))
}

impl EditorPluginEditor for AssetViewerPlugin {
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
        registry.register_fn(EditorId::new("asset-viewer"), |file_path, window, cx| {
            self.create_editor(file_path, window, cx)
        });
    }
}

impl EditorPluginStatusbar for AssetViewerPlugin {}
impl EditorPluginAi for AssetViewerPlugin {}
impl EditorPluginComponents for AssetViewerPlugin {
    fn component_definitions(&self) -> Vec<ComponentDefinition> {
        Vec::new()
    }
}
impl AssetViewerPlugin {
    fn create_editor(
        &'static self,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        log::info!("Creating asset viewer for {:?}", file_path);

        let panel =
            cx.new(|cx| editor::panel::AssetViewerPanel::new(file_path.clone(), window, cx));

        log::info!("Created asset viewer for {:?}", file_path);
        Ok(Arc::new(panel))
    }
}

// Static built-ins are instantiated through the Rust API and must not emit the
// process-global dynamic loader symbols shared by every plugin.
#[cfg(not(feature = "builtin"))]
export_plugin!(AssetViewerPlugin);

// This plugin's descriptor (`pulsar-plugin.toml`): linking the crate installs it.
pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// Everything the plugin registers (the editor provider, with the `builtin`
/// feature).
#[cfg_attr(not(feature = "builtin"), allow(unused_variables))]
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    examples::build(p);
    #[cfg(feature = "builtin")]
    builtin_provider::build(p);
}

/// The plugin's example files, served as `examples/` in its folder.
mod examples;

/// The plugin's editor provider, registered by `build` (origin-gated).
#[cfg(feature = "builtin")]
mod builtin_provider;
