//! Built-in editor registration system.
//!
//! This module provides a unified interface for registering built-in editors
//! using the same plugin architecture, but without DLL loading complexity.
//!
//! Built-in editors implement the same traits as plugins, making it easy to
//! migrate them to DLLs later when the system is more stable.

use crate::{EditorRegistry, FileTypeRegistry};
use plugin_editor_api::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// Import needed for PanelView trait and GPUI types
use gpui::{App, Window};
use ui::dock::PanelView;

/// Context provided to editors during creation, containing engine-level information.
pub struct EditorContext {
    /// The current project root path, if any.
    pub project_root: Option<PathBuf>,
}

impl EditorContext {
    pub fn new(project_root: Option<PathBuf>) -> Self {
        Self { project_root }
    }
}

/// Trait for built-in editor providers.
///
/// This trait allows built-in editors to be treated the same as plugin editors,
/// but without the DLL loading complexity.
pub trait BuiltinEditorProvider: Send + Sync {
    fn provider_id(&self) -> &str;
    fn file_types(&self) -> Vec<FileTypeDefinition>;
    fn editors(&self) -> Vec<EditorMetadata>;
    fn can_handle(&self, editor_id: &EditorId) -> bool;

    /// AI tools exposed by this built-in provider.
    fn ai_tools(&self) -> Vec<AiToolDefinition> {
        Vec::new()
    }

    /// File-specific capabilities for selecting tools.
    fn capabilities_for_file(&self, _file_path: &Path) -> Vec<String> {
        Vec::new()
    }

    /// Execute an AI tool exposed by this built-in provider.
    fn execute_ai_tool(
        &self,
        file_path: &Path,
        tool_name: &str,
        tool_args: JsonValue,
    ) -> Result<JsonValue, PluginError> {
        let _ = (file_path, tool_name, tool_args);
        Err(PluginError::Other {
            message: "Tool execution not supported by this built-in provider".to_string(),
        })
    }

    /// Create editor directly - each provider implements this with their specific editor type.
    fn create_editor(
        &self,
        file_path: PathBuf,
        editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<std::sync::Arc<dyn PanelView>, PluginError>;

    /// Get component definitions provided by this provider (optional).
    /// Override this to register custom engine components.
    fn component_definitions(&self) -> Vec<ComponentDefinition> {
        Vec::new()
    }
}

/// Registry for all built-in editors.
pub struct BuiltinEditorRegistry {
    providers: Vec<Arc<dyn BuiltinEditorProvider>>,
}

impl BuiltinEditorRegistry {
    /// Create a new registry (empty - providers must be registered).
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
        }
    }

    /// Register a built-in editor provider.
    pub fn register_provider(&mut self, provider: Arc<dyn BuiltinEditorProvider>) {
        self.providers.push(provider);
    }

    /// Get all registered built-in providers.
    pub fn providers(&self) -> &[Arc<dyn BuiltinEditorProvider>] {
        &self.providers
    }

    /// Get a built-in provider by provider id.
    pub fn provider_by_id(&self, provider_id: &str) -> Option<&Arc<dyn BuiltinEditorProvider>> {
        self.providers
            .iter()
            .find(|provider| provider.provider_id() == provider_id)
    }

    /// Get all component definitions from all built-in providers.
    pub fn get_all_components(&self) -> Vec<(PluginId, ComponentDefinition)> {
        let mut components = Vec::new();
        for provider in &self.providers {
            let plugin_id = PluginId::new(provider.provider_id());
            for def in provider.component_definitions() {
                components.push((plugin_id.clone(), def));
            }
        }
        components
    }

    /// Register all built-in editors with the file type and editor registries.
    pub fn register_all(
        &self,
        file_type_registry: &mut FileTypeRegistry,
        editor_registry: &mut EditorRegistry,
    ) {
        let builtin_plugin_id = PluginId::new("builtin");

        for provider in &self.providers {
            tracing::info!("Registering built-in provider: {}", provider.provider_id());

            for file_type in provider.file_types() {
                tracing::debug!("  - Registering file type: {}", file_type.id);
                file_type_registry.register(file_type, builtin_plugin_id.clone());
            }

            for editor in provider.editors() {
                tracing::debug!("  - Registering editor: {}", editor.id);
                editor_registry.register(editor, builtin_plugin_id.clone());
            }
        }

        tracing::info!(
            "Registered {} built-in editor providers",
            self.providers.len()
        );
    }

    /// Create an editor using the appropriate provider.
    pub fn create_editor(
        &self,
        editor_id: &EditorId,
        file_path: PathBuf,
        editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<std::sync::Arc<dyn PanelView>, PluginError> {
        for provider in &self.providers {
            if provider.can_handle(editor_id) {
                return provider.create_editor(file_path, editor_context, window, cx);
            }
        }

        Err(PluginError::Other {
            message: format!("No built-in editor found for: {}", editor_id),
        })
    }
}

impl Default for BuiltinEditorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
