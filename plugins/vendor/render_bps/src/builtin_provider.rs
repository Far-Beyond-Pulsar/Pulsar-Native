//! The Render Graph editor as a built-in editor provider.
//!
//! Built into a binary (feature `builtin`) the plugin registers itself at link
//! time through `plugin_manager::LinkedEditorProvider`, as the Blueprint editor
//! does. The two share one copy of the graph editor; each document takes the
//! profile that claims its extension, so they do not interfere.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, AppContext, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext, LinkedEditorProvider};
use ui::dock::PanelView;

use crate::{ensure_profile, file_type, EDITOR, FILE_TYPE};

pub struct RenderBpsBuiltinProvider;

impl BuiltinEditorProvider for RenderBpsBuiltinProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.render-bps"
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

    fn can_handle(&self, editor_id: &EditorId) -> bool {
        editor_id.as_str() == EDITOR
    }

    fn create_editor(
        &self,
        file_path: PathBuf,
        editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        ensure_profile(Some(&*window));
        let panel = cx.new(|cx| {
            match blueprint_editor_plugin::BlueprintEditorPanel::new_with_path(
                file_path.clone(),
                window,
                cx,
            ) {
                Ok(panel) => panel,
                Err(error) => {
                    tracing::error!("Failed to open render graph {file_path:?}: {error}");
                    blueprint_editor_plugin::BlueprintEditorPanel::new_with_load_error(
                        file_path.clone(),
                        error.to_string(),
                        window,
                        cx,
                    )
                }
            }
        });
        if let Some(project_root) = editor_context.project_root.clone() {
            panel.update(cx, |panel, _cx| {
                panel.project_root = Some(project_root);
            });
        }
        let undo = editor_context.services.undo.clone();
        panel.update(cx, |panel, cx| panel.set_undo_service(undo, cx));

        Ok(Arc::new(panel))
    }
}

plugin_manager::inventory::submit! {
    LinkedEditorProvider { create: || Arc::new(RenderBpsBuiltinProvider) }
}
