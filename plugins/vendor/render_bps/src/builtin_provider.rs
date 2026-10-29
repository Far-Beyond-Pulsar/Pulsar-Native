//! The Render Graph editor as a built-in editor provider.
//!
//! Built into a binary (feature `builtin`) the plugin registers itself at link
//! time through `plugin_manager::LinkedEditorProvider`, as the Blueprint editor
//! does. The two share nothing: each opens its own editor over its own documents.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Window};
use plugin_editor_api::*;
use plugin_manager::{BuiltinEditorProvider, EditorContext, LinkedEditorProvider};
use ui::dock::PanelView;

use crate::{file_type, open_editor, EDITOR, FILE_TYPE};

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
        let panel = open_editor(file_path, window, cx);
        let undo = editor_context.services.undo.clone();
        panel.update(cx, |panel, _cx| panel.set_undo_service(undo));
        Ok(Arc::new(panel))
    }
}

plugin_manager::inventory::submit! {
    LinkedEditorProvider { create: || Arc::new(RenderBpsBuiltinProvider) }
}
