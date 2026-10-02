//! The language, and its link-time registration.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Window};
use plugin_editor_api::{
    AiToolDefinition, CompileDiagnostic, EditorId, EditorMetadata, FileTypeDefinition, NativeRegistry, PluginError, ScriptLanguage,
};
use plugin_manager::{BuiltinEditorProvider, EditorContext, LinkedEditorProvider};
use ui::dock::PanelView;

use crate::compile::{compile_project_classes, validate_project_classes, LANGUAGE_ID};

pub struct TypeScriptLanguage;

impl ScriptLanguage for TypeScriptLanguage {
    fn id(&self) -> &str {
        LANGUAGE_ID
    }

    fn display_name(&self) -> &str {
        "TypeScript"
    }

    fn validate_project(&self, project_root: &Path) -> Result<(), String> {
        validate_project_classes(project_root, &NativeRegistry::with_engine_natives())
    }

    fn compile_project(&self, project_root: &Path, natives: &NativeRegistry) -> Vec<CompileDiagnostic> {
        compile_project_classes(project_root, natives)
    }
}

/// The TypeScript scripting language, for hosts that embed this plugin.
pub fn script_language() -> Arc<dyn ScriptLanguage> {
    Arc::new(TypeScriptLanguage)
}

/// Contributes the language to the editor. TypeScript has no editor of its
/// own (the code editor edits `.ts`), so there are no file types or editors.
struct TypeScriptProvider;

impl BuiltinEditorProvider for TypeScriptProvider {
    fn provider_id(&self) -> &str {
        "com.pulsar.typescript-language"
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        Vec::new()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        Vec::new()
    }

    fn can_handle(&self, _: &EditorId) -> bool {
        false
    }

    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        Vec::new()
    }

    fn script_languages(&self) -> Vec<Arc<dyn ScriptLanguage>> {
        vec![script_language()]
    }

    fn create_editor(&self, _: PathBuf, _: &EditorContext, _: &mut Window, _: &mut App) -> Result<Arc<dyn PanelView>, PluginError> {
        Err(PluginError::Other { message: "TypeScript has no editor panel".into() })
    }
}

plugin_manager::inventory::submit! {
    LinkedEditorProvider { create: || Arc::new(TypeScriptProvider) }
}

plugin_editor_api::inventory::submit! {
    plugin_editor_api::LinkedScriptLanguage { create: script_language }
}
