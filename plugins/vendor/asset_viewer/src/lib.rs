use gpui::*;
use plugin_editor_api::*;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::{path::PathBuf, sync::Arc};
use ui::dock::PanelView;

rust_i18n::i18n!("locales", fallback = "en");

mod editor;
pub use editor::panel::AssetViewerPanel;

struct EditorStorage {
    panel: Arc<dyn PanelView>,
}

pub struct AssetViewerPlugin {
    editors: Arc<Mutex<HashMap<usize, EditorStorage>>>,
    next_editor_id: Arc<Mutex<usize>>,
}

impl Default for AssetViewerPlugin {
    fn default() -> Self {
        Self {
            editors: Arc::new(Mutex::new(HashMap::new())),
            next_editor_id: Arc::new(Mutex::new(0)),
        }
    }
}

impl EditorPlugin for AssetViewerPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.pulsar.asset-viewer"),
            name: t!("AssetViewer.DisplayName").into(),
            version: "0.1.0".into(),
            author: t!("AssetViewer.Team").into(),
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
                categories: vec![t!("AssetViewer.Images").to_string()],
            },
        ]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("asset-viewer"),
            display_name: "Asset Viewer".into(),
            supported_file_types: vec![FileTypeId::new("fbx"), FileTypeId::new("png")],
        }]
    }
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
impl EditorPluginSubsystems for AssetViewerPlugin {
    fn subsystems(&self) -> Vec<Box<dyn Subsystem>> {
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

        let panel = cx.new(|cx| {
            editor::panel::AssetViewerPanel::new(file_path.clone(), window, cx)
        });

        let panel_arc: Arc<dyn PanelView> = Arc::new(panel.clone());

        let id = {
            let mut next_id = self.next_editor_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };

        self.editors
            .lock()
            .unwrap()
            .insert(id, EditorStorage { panel: panel_arc.clone() });

        log::info!("Created asset viewer instance {} for {:?}", id, file_path);
        Ok(panel_arc)
    }
}

// Static built-ins are instantiated through the Rust API and must not emit the
// process-global dynamic loader symbols shared by every plugin.
#[cfg(not(feature = "builtin"))]
export_plugin!(AssetViewerPlugin);
