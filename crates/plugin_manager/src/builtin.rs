//! Built-in editor registration system.
//!
//! This module provides a unified interface for registering built-in editors
//! using the same plugin architecture, but without DLL loading complexity.
//!
//! Built-in editors implement the same traits as plugins, making it easy to
//! migrate them to DLLs later when the system is more stable.

use plugin_editor_api::*;
use crate::{EditorRegistry, FileTypeRegistry};
use std::sync::Arc;
use std::path::PathBuf;

// Import needed for PanelView trait and GPUI types
use ui::dock::PanelView;
use gpui::{Window, App};

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
    
    /// Create editor directly - each provider implements this with their specific editor type.
    fn create_editor(
        &self,
        file_path: PathBuf,
        editor_context: &EditorContext,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<std::sync::Arc<dyn PanelView>, PluginError>;
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
        
        tracing::info!("Registered {} built-in editor providers", self.providers.len());
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
