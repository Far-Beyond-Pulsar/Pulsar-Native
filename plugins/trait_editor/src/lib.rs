//! Trait asset editor for `.trait.json` definitions.
//!
//! The editor owns a structured draft of the canonical `ui_types_common::TraitAsset`
//! schema. It writes through `engine_fs::virtual_fs`, so the same editor works with
//! local and remote project providers.

use gpui::{App, AppContext, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext, LinkedEditorProvider};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dock::PanelView;

mod editor;

pub use editor::TraitEditor;

/// Built-in provider registered through the same link-time registry as the
/// Blueprint editor.
pub struct TraitEditorBuiltinProvider;

impl BuiltinEditorProvider for TraitEditorBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.trait-editor"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        vec![FileTypeDefinition {
            id: FileTypeId::new("trait"),
            extension: "trait.json".to_owned(),
            display_name: "Trait Definition".to_owned(),
            icon: ui::IconName::Code,
            color: gpui::rgb(0x8B7CF6).into(),
            structure: FileStructure::Standalone,
            default_content: serde_json::json!({
                "schemaVersion": 1,
                "typeKind": "trait",
                "name": "NewTrait",
                "displayName": "New Trait",
                "description": null,
                "methods": [],
                "meta": {}
            }),
            categories: vec!["Types".to_owned(), "Traits".to_owned()],
        }]
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![EditorMetadata {
            id: EditorId::new("trait-editor"),
            display_name: "Trait Editor".into(),
            supported_file_types: vec![FileTypeId::new("trait")],
        }]
    }

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == "trait-editor"
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        _editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let panel = cx.new(|cx| TraitEditor::open(file_path, window, cx));
        Ok(Arc::new(panel))
    }
}

plugin_editor_api::inventory::submit! {
    LinkedEditorProvider { create: || Arc::new(TraitEditorBuiltinProvider) }
}

/// The type editor is also useful to project validation and plugin callers.
pub fn load_trait(path: &Path) -> anyhow::Result<ui_types_common::TraitAsset> {
    let bytes = engine_fs::virtual_fs::read_file(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let value = normalize_legacy_trait(value)?;
    Ok(serde_json::from_value(value)?)
}

fn normalize_legacy_trait(mut value: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("trait file must contain a JSON object"))?;

    // The original AssetKind::Trait template predates the canonical type-system
    // envelope. Upgrade only those metadata keys that have unambiguous meaning.
    if !object.contains_key("schemaVersion") {
        object.insert("schemaVersion".into(), serde_json::json!(1));
    }
    if !object.contains_key("typeKind") {
        object.insert("typeKind".into(), serde_json::json!("trait"));
    }
    if !object.contains_key("displayName") {
        if let Some(display_name) = object.remove("display_name") {
            object.insert("displayName".into(), display_name);
        } else if let Some(name) = object.get("name").cloned() {
            object.insert("displayName".into(), name);
        }
    }
    object.remove("visibility");
    if !object.contains_key("meta") {
        object.insert("meta".into(), serde_json::json!({}));
    }
    Ok(value)
}
