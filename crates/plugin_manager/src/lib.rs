//! # Plugin Manager
//!
//! This crate provides the infrastructure for loading, managing, and using editor plugins
//! in the Pulsar engine. It handles:
//!
//! - WASM module loading from `plugins/editor/`
//! - File type and editor registration
//! - Editor instance creation
//! - Plugin lifecycle management
//!
//! ## Usage
//!
//! ```rust,ignore
//! use plugin_manager::PluginManager;
//!
//! // Create and initialize the plugin manager
//! let mut manager = PluginManager::new();
//! manager.load_plugins_from_dir("plugins/editor")?;
//!
//! // Query available file types
//! let file_types = manager.get_all_file_types();
//!
//! // Create an editor for a file
//! let editor = manager.create_editor_for_file(
//!     &file_path,
//!     window,
//!     cx,
//! )?;
//! ```
//!
//! ## WASM Plugin System
//!
//! Plugins are compiled to WebAssembly and run in a sandboxed environment using Wasmtime.
//! This provides:
//!
//! - Memory safety (no heap corruption across boundaries)
//! - Platform independence (same .wasm works on Windows/Linux/macOS)
//! - Automatic cleanup (WASM linear memory is managed)
//! - Security isolation

use plugin_editor_api::*;
use ui::dock::PanelView;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use once_cell::sync::OnceCell;

mod registry;
pub mod builtin;
pub mod wasm;

pub use registry::{EditorRegistry, FileTypeRegistry};
pub use builtin::{BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext};
pub use wasm::{WasmPluginHost, WasmPlugin};

// ============================================================================
// Global Plugin Manager
// ============================================================================

/// Global plugin manager instance
static GLOBAL_PLUGIN_MANAGER: OnceCell<RwLock<PluginManager>> = OnceCell::new();

/// Initialize the global plugin manager
/// This should be called once at application startup
pub fn initialize_global(manager: PluginManager) {
    if GLOBAL_PLUGIN_MANAGER.set(RwLock::new(manager)).is_err() {
        tracing::warn!("Global plugin manager already initialized");
    }
}

/// Get a read-only reference to the global plugin manager
pub fn global() -> Option<&'static RwLock<PluginManager>> {
    GLOBAL_PLUGIN_MANAGER.get()
}

// ============================================================================
// Plugin Manager
// ============================================================================

/// Manages all WASM editor plugins in the system.
/// Uses safe patterns from Zed's WASM host to prevent memory leaks.
pub struct PluginManager {
    /// WASM plugin host (shared across all WASM plugins)
    wasm_host: Arc<WasmPluginHost>,

    /// Loaded WASM plugins indexed by ID
    plugins: HashMap<PluginId, Arc<WasmPlugin>>,

    /// Registry of all file types
    file_type_registry: FileTypeRegistry,

    /// Registry of all editors
    editor_registry: EditorRegistry,
    
    /// Built-in editor registry
    builtin_registry: BuiltinEditorRegistry,
    
    /// Project root path for editor context
    project_root: Option<PathBuf>,

    /// Statusbar buttons registered by plugins
    statusbar_buttons: Vec<(PluginId, StatusbarButtonDefinition)>,
}

impl PluginManager {
    /// Create a new plugin manager.
    pub fn new() -> Self {
        let wasm_host = Arc::new(
            WasmPluginHost::new().expect("Failed to initialize WASM host")
        );
        
        Self {
            wasm_host,
            plugins: HashMap::new(),
            file_type_registry: FileTypeRegistry::new(),
            editor_registry: EditorRegistry::new(),
            builtin_registry: BuiltinEditorRegistry::new(),
            project_root: None,
            statusbar_buttons: Vec::new(),
        }
    }
    
    /// Set the project root path for editor context.
    pub fn set_project_root(&mut self, project_root: Option<PathBuf>) {
        self.project_root = project_root;
    }
    
    /// Get a mutable reference to the built-in editor registry.
    ///
    /// This allows external code to register built-in editors during initialization.
    pub fn builtin_registry_mut(&mut self) -> &mut BuiltinEditorRegistry {
        &mut self.builtin_registry
    }
    
    /// Register all built-in editors with the file type and editor registries.
    ///
    /// This should be called after all built-in editors have been registered
    /// with the builtin registry.
    pub fn register_builtin_editors(&mut self) {
        self.builtin_registry.register_all(
            &mut self.file_type_registry,
            &mut self.editor_registry,
        );
    }

    /// Load all plugins from a directory.
    ///
    /// This will scan the directory for WASM modules (.wasm) and attempt to load each one as a plugin.
    ///
    /// Plugins that fail loading will be logged but won't prevent other plugins from loading.
    pub fn load_plugins_from_dir(&mut self, dir: impl AsRef<Path>, cx: &gpui::App) -> Result<(), PluginManagerError> {
        let dir = dir.as_ref();

        if !dir.exists() {
            tracing::warn!("Plugin directory does not exist: {:?}", dir);
            return Ok(());
        }

        tracing::debug!("Loading WASM plugins from: {:?}", dir);

        // Scan directory for WASM modules
        for entry in walkdir::WalkDir::new(dir)
            .max_depth(2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            // Check if this is a WASM file
            if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
                continue;
            }

            // Attempt to load the plugin
            match self.load_wasm_plugin(path, cx) {
                Ok(plugin_id) => {
                    tracing::info!("✓ Successfully loaded plugin: {}", plugin_id);
                }
                Err(e) => {
                    tracing::error!("Failed to load plugin from {:?}: {}", path, e);
                }
            }
        }

        Ok(())
    }

    /// Load a single plugin from a library file.
    ///
    /// # Safety
    ///
    /// This function loads and executes code from a dynamic library or WASM module.
    /// For DLL plugins: The library must be compiled with the same Rust version and
    /// for the same engine version as the current build.
    /// For WASM plugins: Uses wasmtime sandboxing for memory safety.
    // Removed: DLL loading code - all plugins are now WASM

    
    fn load_wasm_plugin(&mut self, path: impl AsRef<Path>, _cx: &gpui::App) -> Result<PluginId, PluginManagerError> {
        let path = path.as_ref();

        tracing::info!("Loading WASM plugin from: {:?}", path);

        // Load the WASM plugin using the host
        let wasm_plugin = futures::executor::block_on(async {
            self.wasm_host.load_plugin(path).await
        }).map_err(|e| PluginManagerError::LibraryLoadError {
            path: path.to_path_buf(),
            message: format!("WASM loading failed: {}", e),
        })?;

        let metadata = wasm_plugin.metadata().clone();
        let plugin_id = metadata.id.clone();

        tracing::info!("Loaded WASM plugin: {} v{}", metadata.name, metadata.version);

        // Register file types
        let file_types = futures::executor::block_on(async {
            wasm_plugin.file_types().await
        }).map_err(|e| PluginManagerError::LibraryLoadError {
            path: path.to_path_buf(),
            message: format!("Failed to get file types: {}", e),
        })?;

        for ft in file_types {
            tracing::debug!("  Registering file type: {} ({})", ft.display_name, ft.extension);
            self.file_type_registry.register(ft, plugin_id.clone());
        }

        // Register editors
        let editors = futures::executor::block_on(async {
            wasm_plugin.editors().await
        }).map_err(|e| PluginManagerError::LibraryLoadError {
            path: path.to_path_buf(),
            message: format!("Failed to get editors: {}", e),
        })?;

        for editor in editors {
            tracing::debug!("  Registering editor: {}", editor.display_name);
            self.editor_registry.register(editor, plugin_id.clone());
        }

        // Store the plugin with Arc for safe reference counting
        self.plugins.insert(plugin_id.clone(), Arc::new(wasm_plugin));

        tracing::info!("✓ WASM plugin {} registered successfully", plugin_id);

        Ok(plugin_id)
    }

    /// Unload a plugin by ID (safe with Arc reference counting)
    pub fn unload_plugin(&mut self, plugin_id: &PluginId) -> Result<(), PluginManagerError> {
        if let Some(plugin) = self.plugins.remove(plugin_id) {
            tracing::info!("Unloading WASM plugin: {}", plugin_id);

            // Call on_unload hook
            futures::executor::block_on(async {
                plugin.on_unload().await
            }).map_err(|e| PluginManagerError::PluginError {
                message: format!("on_unload failed: {}", e),
            })?;

            // Remove file types
            self.file_type_registry.unregister_by_plugin(plugin_id);

            // Remove editors
            self.editor_registry.unregister_by_plugin(plugin_id);

            // Remove statusbar buttons
            let buttons_before = self.statusbar_buttons.len();
            self.statusbar_buttons.retain(|(owner_id, _)| owner_id != plugin_id);
            let buttons_removed = buttons_before - self.statusbar_buttons.len();
            if buttons_removed > 0 {
                tracing::debug!("Removed {} statusbar buttons from plugin", buttons_removed);
            }

            // Plugin is dropped here, Arc ensures no memory leaks
            // WASM linear memory is automatically cleaned up by wasmtime
            tracing::info!("✓ Plugin {} unloaded successfully", plugin_id);
            Ok(())
        } else {
            Err(PluginManagerError::PluginNotFound {
                plugin_id: plugin_id.clone(),
            })
        }
    }

    /// Get all loaded plugins
    pub fn get_plugins(&self) -> Vec<&PluginMetadata> {
        self.plugins.values().map(|p| p.metadata()).collect()
    }

    /// Get a reference to the file type registry.
    pub fn file_type_registry(&self) -> &FileTypeRegistry {
        &self.file_type_registry
    }

    /// Get a reference to the editor registry.
    pub fn editor_registry(&self) -> &EditorRegistry {
        &self.editor_registry
    }
    
    /// Get all registered statusbar buttons from all plugins.
    ///
    /// Buttons are sorted by position (left/right) and priority within each position.
    /// Higher priority buttons appear first in their respective position group.
    ///
    /// Returns only the button definitions, not the plugin IDs.
    pub fn get_statusbar_buttons(&self) -> Vec<&StatusbarButtonDefinition> {
        self.statusbar_buttons.iter().map(|(_, btn)| btn).collect()
    }

    /// Get statusbar buttons for a specific position.
    pub fn get_statusbar_buttons_for_position(&self, position: StatusbarPosition) -> Vec<&StatusbarButtonDefinition> {
        self.statusbar_buttons
            .iter()
            .filter(|(_, btn)| btn.position == position)
            .map(|(_, btn)| btn)
            .collect()
    }
    
    /// Find which plugin owns a given editor ID
    pub fn find_plugin_for_editor(&self, editor_id: &EditorId) -> Option<PluginId> {
        for (plugin_id, plugin) in &self.plugins {
            let editors = futures::executor::block_on(async {
                plugin.editors().await
            });
            
            if let Ok(editors) = editors {
                if editors.iter().any(|e| &e.id == editor_id) {
                    return Some(plugin_id.clone());
                }
            }
        }
        None
    }

    /// Create an editor instance for a file.
    ///
    /// This will:
    /// 1. Determine the file type from the path
    /// 2. Find an editor that supports that file type (if any)
    /// 3. Create an editor instance using the appropriate plugin
    ///      else
    ///    We will return an error in a notification if no suitable
    ///    editor is found. TODO: Implement a suggested plugins system
    ///    that can scan the plugins dir on request to identify plugin
    ///    that may provide support for the file type.
    /// Create an editor for a file by detecting its type and finding an appropriate editor.
    ///
    /// This method:
    /// 1. Tries built-in editors first (fast, native code)
    /// 2. Falls back to WASM plugins if no built-in editor is found
    /// 3. Returns error if no editor can be found
    pub fn create_editor_for_file(
        &mut self,
        file_path: &Path,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(std::sync::Arc<dyn ui::dock::PanelView>, Box<dyn EditorInstance>), PluginManagerError> {
        // Determine file type
        let file_type_id = self
            .file_type_registry
            .get_file_type_for_path(file_path)
            .ok_or_else(|| PluginManagerError::NoFileTypeForPath {
                path: file_path.to_path_buf(),
            })?;

        // Find an editor for this file type
        let editor_id = self
            .editor_registry
            .get_editor_for_file_type(&file_type_id)
            .ok_or_else(|| PluginManagerError::NoEditorForFileType {
                file_type_id: file_type_id.clone(),
            })?;

        // Get the plugin that owns this editor
        let plugin_id = self
            .editor_registry
            .get_plugin_for_editor(&editor_id)
            .ok_or_else(|| PluginManagerError::EditorNotFound {
                editor_id: editor_id.clone(),
            })?
            .clone(); // Clone to avoid borrow checker issues

        // Check if this is a built-in editor (plugin_id == "builtin")
        if plugin_id.as_str() == "builtin" {
            // Create editor context with project root
            let editor_context = crate::builtin::EditorContext::new(self.project_root.clone());
            
            // Create the editor directly using the provider
            match self.builtin_registry.create_editor(
                &editor_id,
                file_path.to_path_buf(),
                &editor_context,
                window,
                cx,
            ) {
                Ok(panel) => {
                    // Return dummy EditorInstance for built-in editors
                    struct BuiltinEditorInstance;
                    impl EditorInstance for BuiltinEditorInstance {
                        fn file_path(&self) -> &PathBuf {
                            // Built-in editors manage their own file paths internally
                            // This method should not be called for built-in editors
                            panic!("Built-in editors handle file paths internally")
                        }
                        fn save(&mut self, _window: &mut Window, _cx: &mut App) -> Result<(), PluginError> {
                            Ok(())
                        }
                        fn reload(&mut self, _window: &mut Window, _cx: &mut App) -> Result<(), PluginError> {
                            Ok(())
                        }
                        fn is_dirty(&self) -> bool {
                            false
                        }
                        fn as_any(&self) -> &dyn std::any::Any {
                            self
                        }
                    }
                    
                    return Ok((panel, Box::new(BuiltinEditorInstance)));
                }
                Err(e) => {
                    tracing::error!("Built-in editor creation failed: {}", e);
                    return Err(PluginManagerError::PluginError {
                        message: format!("Built-in editor creation failed: {}", e),
                    });
                }
            }
        }

        // Create editor using WASM plugin
        self.create_editor(&plugin_id, &editor_id, file_path.to_path_buf(), window, cx)
    }

    /// Create an editor instance with specific IDs (calls WASM plugin safely)
    pub fn create_editor(
        &mut self,
        plugin_id: &PluginId,
        editor_id: &EditorId,
        file_path: PathBuf,
        _window: &mut Window,
        cx: &mut App,
    ) -> Result<(std::sync::Arc<dyn ui::dock::PanelView>, Box<dyn EditorInstance>), PluginManagerError> {
        // Get plugin with Arc clone for safe reference
        let plugin = self.plugins.get(plugin_id)
            .ok_or_else(|| PluginManagerError::PluginNotFound {
                plugin_id: plugin_id.clone(),
            })?
            .clone(); // Clone the Arc, not the plugin

        // Create editor instance in WASM
        let instance_id = futures::executor::block_on(async {
            plugin.create_editor(editor_id.clone(), file_path.clone()).await
        }).map_err(|e| PluginManagerError::EditorCreationFailed {
            editor_id: editor_id.to_string(),
            message: format!("WASM editor creation failed: {}", e),
        })?;

        // Create a safe wrapper using Arc instead of raw pointers
        struct WasmEditorInstanceWrapper {
            plugin: Arc<WasmPlugin>, // Keep Arc to plugin alive
            instance_id: String,
            file_path: PathBuf,
        }

        impl EditorInstance for WasmEditorInstanceWrapper {
            fn file_path(&self) -> &PathBuf {
                &self.file_path
            }

            fn save(&mut self, _window: &mut Window, _cx: &mut App) -> Result<(), PluginError> {
                // Safe: plugin is kept alive by Arc
                futures::executor::block_on(async {
                    self.plugin.save_editor(&self.instance_id).await
                }).map_err(|e| PluginError::Other {
                    message: format!("Save failed: {}", e),
                })
            }

            fn reload(&mut self, _window: &mut Window, _cx: &mut App) -> Result<(), PluginError> {
                // WASM plugins handle reload internally
                Ok(())
            }

            fn is_dirty(&self) -> bool {
                futures::executor::block_on(async {
                    self.plugin.is_dirty(&self.instance_id).await.unwrap_or(false)
                })
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }

        let wrapper = WasmEditorInstanceWrapper {
            plugin: plugin.clone(), // Keep plugin alive via Arc
            instance_id: instance_id.clone(),
            file_path: file_path.clone(),
        };

        // Create UI panel using GPUI View properly
        use gpui::{EventEmitter, Focusable, Render, IntoElement, Context, FocusHandle, ParentElement, Styled};
        use ui::dock::{Panel, PanelEvent, PanelState};
        
        struct WasmEditorPanel {
            plugin: Arc<WasmPlugin>, // Keep plugin alive
            instance_id: String,
            file_path: PathBuf,
            focus_handle: FocusHandle,
        }
        
        impl WasmEditorPanel {
            fn new(
                plugin: Arc<WasmPlugin>,
                instance_id: String,
                file_path: PathBuf,
                cx: &mut Context<Self>
            ) -> Self {
                Self {
                    plugin,
                    instance_id,
                    file_path,
                    focus_handle: cx.focus_handle(),
                }
            }
        }
        
        impl Panel for WasmEditorPanel {
            fn panel_name(&self) -> &'static str { 
                "wasm-editor" 
            }
            
            fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
                gpui::div()
                    .child(format!("WASM: {}", 
                        self.file_path.file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("Unknown")
                    ))
                    .into_any_element()
            }
            
            fn dump(&self, _cx: &App) -> PanelState {
                PanelState::new(self)
            }
        }
        
        impl EventEmitter<PanelEvent> for WasmEditorPanel {}
        
        impl Focusable for WasmEditorPanel {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus_handle.clone()
            }
        }
        
        impl Render for WasmEditorPanel {
            fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
                // TODO: Call into WASM to get UI description and render it
                // For now, show a placeholder
                gpui::div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child("WASM Editor Panel")
                    .child(format!("Plugin: {}", self.plugin.metadata().name))
                    .child(format!("Instance: {}", self.instance_id))
                    .child(format!("File: {}", self.file_path.display()))
            }
        }
        
        // Create the panel view (App has the new method via AppContext trait)
        let panel_view = gpui::AppContext::new(cx, |cx| {
            WasmEditorPanel::new(plugin, instance_id, file_path, cx)
        });
        
        let panel_arc: Arc<dyn PanelView> = Arc::new(panel_view);

        Ok((panel_arc, Box::new(wrapper)))
    }

    /// Get the default content for a file type.
    pub fn get_default_content(&self, file_type_id: &FileTypeId) -> Option<&serde_json::Value> {
        self.file_type_registry
            .get_file_type(file_type_id)
            .map(|ft| &ft.default_content)
    }

    /// Create a new file of the given type.
    ///
    /// This will create the file structure on disk with default content.
    pub fn create_new_file(
        &self,
        file_type_id: &FileTypeId,
        path: &Path,
    ) -> Result<(), PluginManagerError> {
        let file_type = self
            .file_type_registry
            .get_file_type(file_type_id)
            .ok_or_else(|| PluginManagerError::FileTypeNotFound {
                file_type_id: file_type_id.clone(),
            })?;

        match &file_type.structure {
            FileStructure::Standalone => {
                // Create a simple file with default content
                let content = serde_json::to_string_pretty(&file_type.default_content)
                    .map_err(|e| PluginManagerError::FileCreationError {
                        path: path.to_path_buf(),
                        message: e.to_string(),
                    })?;

                std::fs::write(path, content).map_err(|e| PluginManagerError::FileCreationError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;
            }

            FileStructure::FolderBased {
                marker_file,
                template_structure,
            } => {
                // Create the folder
                std::fs::create_dir_all(path).map_err(|e| PluginManagerError::FileCreationError {
                    path: path.to_path_buf(),
                    message: e.to_string(),
                })?;

                // Create the marker file with default content
                let marker_path = path.join(marker_file);
                let content = serde_json::to_string_pretty(&file_type.default_content)
                    .map_err(|e| PluginManagerError::FileCreationError {
                        path: marker_path.clone(),
                        message: e.to_string(),
                    })?;

                std::fs::write(&marker_path, content).map_err(|e| {
                    PluginManagerError::FileCreationError {
                        path: marker_path,
                        message: e.to_string(),
                    }
                })?;

                // Create template structure
                for template in template_structure {
                    match template {
                        PathTemplate::File { path: rel_path, content } => {
                            let file_path = path.join(rel_path);
                            if let Some(parent) = file_path.parent() {
                                std::fs::create_dir_all(parent).ok();
                            }
                            std::fs::write(&file_path, content).map_err(|e| {
                                PluginManagerError::FileCreationError {
                                    path: file_path,
                                    message: e.to_string(),
                                }
                            })?;
                        }
                        PathTemplate::Folder { path: rel_path } => {
                            let folder_path = path.join(rel_path);
                            std::fs::create_dir_all(&folder_path).map_err(|e| {
                                PluginManagerError::FileCreationError {
                                    path: folder_path,
                                    message: e.to_string(),
                                }
                            })?;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

// WASM plugins have automatic cleanup
impl Drop for PluginManager {
    fn drop(&mut self) {
        tracing::debug!("PluginManager dropped, WASM runtime will clean up");
    }
}

// ============================================================================
// Plugin Manager Errors
// ============================================================================

/// Errors that can occur in the plugin manager.
#[derive(Debug, Clone)]
pub enum PluginManagerError {
    /// Failed to load WASM module
    LibraryLoadError { path: PathBuf, message: String },

    /// Plugin not found
    PluginNotFound { plugin_id: PluginId },

    /// File type not found
    FileTypeNotFound { file_type_id: FileTypeId },

    /// Editor not found
    EditorNotFound { editor_id: EditorId },

    /// No file type for path
    NoFileTypeForPath { path: PathBuf },

    /// No editor for file type
    NoEditorForFileType { file_type_id: FileTypeId },

    /// Plugin error (generic)
    PluginError { message: String },

    /// Failed to create file
    FileCreationError { path: PathBuf, message: String },

    /// Failed to create editor
    EditorCreationFailed { editor_id: String, message: String },
}

impl std::fmt::Display for PluginManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryLoadError { path, message } => {
                write!(f, "Failed to load WASM module {:?}: {}", path, message)
            }
            Self::PluginNotFound { plugin_id } => {
                write!(f, "Plugin not found: {}", plugin_id)
            }
            Self::FileTypeNotFound { file_type_id } => {
                write!(f, "File type not found: {}", file_type_id)
            }
            Self::EditorNotFound { editor_id } => {
                write!(f, "Editor not found: {}", editor_id)
            }
            Self::NoFileTypeForPath { path } => {
                write!(f, "No file type registered for path: {:?}", path)
            }
            Self::NoEditorForFileType { file_type_id } => {
                write!(f, "No editor registered for file type: {}", file_type_id)
            }
            Self::PluginError { message } => {
                write!(f, "Plugin error: {}", message)
            }
            Self::FileCreationError { path, message } => {
                write!(f, "Failed to create file {:?}: {}", path, message)
            }
            Self::EditorCreationFailed { editor_id, message } => {
                write!(f, "Failed to create editor {}: {}", editor_id, message)
            }
        }
    }
}

impl std::error::Error for PluginManagerError {}
