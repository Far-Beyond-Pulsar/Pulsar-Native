//! # Plugin Manager
//!
//! This crate provides the infrastructure for loading, managing, and using editor plugins
//! in the Pulsar engine. It handles:
//!
//! - Dynamic library loading from `plugins/editor/`
//! - Version compatibility checking
//! - File type and editor registration
//! - Editor instance creation
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
//! ## Safety and Memory Management
//!
//! This plugin system uses dynamic library loading with careful memory management to avoid
//! heap corruption across DLL boundaries:
//!
//! ### Memory Ownership Rules
//!
//! 1. **Plugin owns its memory**: All plugin instances (`Box<dyn EditorPlugin>`) are allocated
//!    in the plugin's heap and MUST be freed by calling the plugin's `_plugin_destroy` function.
//!
//! 2. **Never use Rust's Drop**: The main app stores raw pointers to plugin instances and
//!    NEVER converts them to `Box` or lets Rust's Drop trait handle cleanup.
//!
//! 3. **Explicit destruction**: Plugins are destroyed by calling their exported `_plugin_destroy`
//!    function, which frees memory in the plugin's heap.
//!
//! ### Cross-DLL Contracts
//!
//! 1. **Theme Pointer Validity**: The main app passes a Theme pointer to plugins. This pointer
//!    MUST remain valid (not moved or dropped) for the entire plugin lifetime.
//!
//! 2. **Function Pointer Stability**: Plugins register function pointers with the main app.
//!    These MUST remain valid until the plugin is unloaded.
//!
//! 3. **ABI Compatibility**: Plugins are checked for ABI version compatibility. Mismatched
//!    versions are rejected at load time.
//!
//! ### Why This Approach
//!
//! On Windows, each DLL has its own heap allocator. Allocating memory in one DLL and freeing
//! it in another causes heap corruption. By ensuring each side frees its own memory, we
//! maintain safety across the DLL boundary.

use libloading::{Library, Symbol};
use plugin_editor_api::*;
use ui::dock::PanelView;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod registry;
pub mod builtin;

pub use registry::{EditorRegistry, FileTypeRegistry};
pub use builtin::{BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext};

// ============================================================================
// Plugin Container
// ============================================================================

/// A loaded plugin with its library handle.
struct LoadedPlugin {
    /// Raw pointer to plugin instance (owned by plugin DLL, not by us!)
    /// SAFETY: This pointer is allocated in the plugin's heap and MUST be freed
    /// by calling the plugin's _plugin_destroy function, NOT by Rust's Drop.
    plugin_ptr: *mut dyn EditorPlugin,

    /// Function to destroy the plugin (frees memory in plugin's heap)
    destroy_fn: PluginDestroy,

    /// The dynamic library handle (must be kept alive)
    #[allow(dead_code)]
    library: Arc<Library>,

    /// Metadata for quick access (owned by main app)
    metadata: PluginMetadata,
}

// ============================================================================
// Plugin Manager
// ============================================================================

/// Manages all editor plugins in the system.
///
/// The PluginManager is responsible for:
/// - Loading plugins from disk
/// - Verifying version compatibility
/// - Maintaining registries of file types and editors
/// - Creating editor instances on demand
/// - Managing built-in editors (same trait interface, no DLL loading)
/// - Managing statusbar buttons registered by plugins
pub struct PluginManager {
    /// All loaded plugins, indexed by plugin ID
    plugins: HashMap<PluginId, LoadedPlugin>,

    /// Registry of all file types
    file_type_registry: FileTypeRegistry,

    /// Registry of all editors
    editor_registry: EditorRegistry,
    
    /// Built-in editor registry (no DLL loading)
    builtin_registry: BuiltinEditorRegistry,

    /// The version info for this engine build
    engine_version: VersionInfo,
    
    /// Project root path for editor context
    project_root: Option<PathBuf>,

    /// Statusbar buttons registered by all plugins
    /// Stored with plugin ownership tracking for proper cleanup
    statusbar_buttons: Vec<(PluginId, StatusbarButtonDefinition)>,
}

impl PluginManager {
    /// Create a new plugin manager.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            file_type_registry: FileTypeRegistry::new(),
            editor_registry: EditorRegistry::new(),
            builtin_registry: BuiltinEditorRegistry::new(),
            engine_version: VersionInfo::current(),
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
    /// This will scan the directory for dynamic libraries (.dll on Windows,
    /// .so on Linux, .dylib on macOS) and attempt to load each one as a plugin.
    ///
    /// Plugins that fail version checks or loading will be logged but won't
    /// prevent other plugins from loading.
    pub fn load_plugins_from_dir(&mut self, dir: impl AsRef<Path>, cx: &gpui::App) -> Result<(), PluginManagerError> {
        let dir = dir.as_ref();

        if !dir.exists() {
            tracing::warn!("Plugin directory does not exist: {:?}", dir);
            return Ok(());
        }

        tracing::debug!("Loading plugins from: {:?}", dir);

        // Get the appropriate file extension for this platform
        #[cfg(target_os = "windows")]
        let extension = "dll";
        #[cfg(target_os = "linux")]
        let extension = "so";
        #[cfg(target_os = "macos")]
        let extension = "dylib";

        // Scan directory for plugin libraries
        for entry in walkdir::WalkDir::new(dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            // Check if this is a dynamic library
            if path.extension().and_then(|s| s.to_str()) != Some(extension) {
                continue;
            }

            // Attempt to load the plugin
            match self.load_plugin(path, cx) {
                Ok(plugin_id) => {
                    tracing::debug!("Successfully loaded plugin: {}", plugin_id);
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
    /// This function loads and executes code from a dynamic library. The library
    /// must be compiled with the same Rust version and for the same engine version
    /// as the current build.
    pub fn load_plugin(&mut self, path: impl AsRef<Path>, cx: &gpui::App) -> Result<PluginId, PluginManagerError> {
        let path = path.as_ref();

        tracing::debug!("Loading plugin from: {:?}", path);

        // Load the library
        let library = unsafe {
            Library::new(path).map_err(|e| PluginManagerError::LibraryLoadError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?
        };

        let library = Arc::new(library);

        // Get the version info function
        let version_fn: Symbol<extern "C" fn() -> VersionInfo> = unsafe {
            library
                .get(b"_plugin_version")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_version".to_string(),
                    message: e.to_string(),
                })?
        };

        // Check version compatibility
        let plugin_version = version_fn();

        tracing::debug!(
            "Version check - Engine: {:?}, Plugin: {:?}",
            self.engine_version,
            plugin_version
        );

        if !self.engine_version.is_compatible(&plugin_version) {
            tracing::error!(
                "Plugin version mismatch! Expected engine v{}.{}.{} (ABI v{}), got v{}.{}.{} (ABI v{})",
                self.engine_version.engine_version.0,
                self.engine_version.engine_version.1,
                self.engine_version.engine_version.2,
                self.engine_version.rustc_version_hash,
                plugin_version.engine_version.0,
                plugin_version.engine_version.1,
                plugin_version.engine_version.2,
                plugin_version.rustc_version_hash,
            );

            return Err(PluginManagerError::VersionMismatch {
                expected: self.engine_version,
                actual: plugin_version,
            });
        }

        tracing::debug!("Version check passed for plugin at {:?}", path);

        // Setup the plugin logger
        // Get the plugin constructor
        let create_fn: Symbol<PluginCreate> = unsafe {
            library
                .get(b"_plugin_create")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_create".to_string(),
                    message: e.to_string(),
                })?
        };

        // Get the plugin destructor (CRITICAL for proper memory management)
        let destroy_fn: Symbol<PluginDestroy> = unsafe {
            library
                .get(b"_plugin_destroy")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_destroy".to_string(),
                    message: e.to_string(),
                })?
        };

        // Create the plugin instance with Theme pointer for cross-DLL global state sync
        // SAFETY: We store the raw pointer and NEVER convert it to Box in main app.
        // The plugin owns this memory and will free it via _plugin_destroy.
        let plugin_ptr = unsafe {
            // Get Theme from main app's global state and pass to plugin
            let theme_ptr = ui::theme::Theme::global(cx) as *const _ as *const std::ffi::c_void;
            let raw_plugin = create_fn(theme_ptr);
            if raw_plugin.is_none() {
                return Err(PluginManagerError::PluginCreationFailed {
                    message: "Plugin constructor returned null".to_string(),
                });
            }
            raw_plugin
        };

        let Some(plugin_ptr) = plugin_ptr else {
            return Err(PluginManagerError::PluginCreationFailed {
                message: "Plugin constructor returned null".to_string(),
            })
        };
        

        // Get plugin metadata by temporarily accessing through raw pointer
        // SAFETY: Plugin just created, pointer is valid. We validated it's not null above.
        let metadata = unsafe { (plugin_ptr).metadata() };
        let plugin_id = metadata.id.clone();

        tracing::debug!(
            "Loaded plugin: {} v{} by {}",
            metadata.name,
            metadata.version,
            metadata.author
        );

        // Call on_load hook via raw pointer
        // SAFETY: Plugin just created, pointer is valid, not null
        unsafe { plugin_ptr.on_load() };

        // Validate plugin is still functioning after on_load
        // Some plugins may fail during initialization
        // Register file types via raw pointer
        // SAFETY: Plugin just created, pointer is valid
        let file_types = unsafe { (plugin_ptr).file_types() };
        for file_type in file_types {
            tracing::debug!(
                "  Registering file type: {} (.{})",
                file_type.display_name,
                file_type.extension
            );
            self.file_type_registry.register(file_type, plugin_id.clone());
        }

        // Register editors via raw pointer
        // SAFETY: Plugin just created, pointer is valid
        let editors = unsafe { (plugin_ptr).editors() };
        for editor in editors {
            tracing::debug!("  Registering editor: {}", editor.display_name);
            self.editor_registry.register(editor, plugin_id.clone());
        }

        // Register statusbar buttons via raw pointer
        // SAFETY: Plugin just created, pointer is valid
        let statusbar_buttons = unsafe { (plugin_ptr).statusbar_buttons() };
        if !statusbar_buttons.is_empty() {
            tracing::debug!("  Registering {} statusbar buttons", statusbar_buttons.len());
            for button in statusbar_buttons {
                tracing::debug!("    - Button: {} at {:?}", button.id, button.position);

                // Store with plugin ID for tracking and cleanup
                self.statusbar_buttons.push((plugin_id.clone(), button));
            }

            // Sort buttons by priority within their position groups
            self.statusbar_buttons.sort_by(|(_, a), (_, b)| {
                match (&a.position, &b.position) {
                    (StatusbarPosition::Left, StatusbarPosition::Left) |
                    (StatusbarPosition::Right, StatusbarPosition::Right) => {
                        b.priority.cmp(&a.priority) // Higher priority comes first
                    }
                    (StatusbarPosition::Left, StatusbarPosition::Right) => std::cmp::Ordering::Less,
                    (StatusbarPosition::Right, StatusbarPosition::Left) => std::cmp::Ordering::Greater,
                }
            });
        }

        // Store the plugin with raw pointer and destroy function
        // CRITICAL: We do NOT take ownership of the plugin memory.
        // The plugin DLL owns it and will free it when destroy_fn is called.
        let loaded_plugin = LoadedPlugin {
            plugin_ptr,
            destroy_fn: *destroy_fn,  // Copy the function pointer
            library,
            metadata: metadata.clone(),
        };

        self.plugins.insert(plugin_id.clone(), loaded_plugin);

        Ok(plugin_id)
    }

    /// Unload a plugin by ID.
    ///
    /// This will call the plugin's `on_unload` hook, destroy the plugin instance
    /// (freeing memory in the plugin's heap), and remove all registered file types and editors.
    pub fn unload_plugin(&mut self, plugin_id: &PluginId) -> Result<(), PluginManagerError> {
        if let Some(loaded_plugin) = self.plugins.remove(plugin_id) {
            // Call on_unload hook via raw pointer
            // SAFETY: Plugin is still valid, about to be destroyed
            unsafe { (*loaded_plugin.plugin_ptr).on_unload() };

            // Remove file types
            self.file_type_registry.unregister_by_plugin(plugin_id);

            // Remove editors
            self.editor_registry.unregister_by_plugin(plugin_id);

            // Remove statusbar buttons registered by this plugin
            // This is critical because button data (especially function pointers in
            // custom_callback) becomes invalid when the plugin DLL is unloaded
            let buttons_before = self.statusbar_buttons.len();
            self.statusbar_buttons.retain(|(owner_id, _)| owner_id != plugin_id);
            let buttons_removed = buttons_before - self.statusbar_buttons.len();
            if buttons_removed > 0 {
                tracing::debug!("Removed {} statusbar buttons from plugin", buttons_removed);
            }

            tracing::debug!("Unloading plugin: {}", loaded_plugin.metadata.name);

            // CRITICAL: Call the plugin's destroy function to free memory in plugin's heap
            // This is the ONLY safe way to free the plugin instance.
            // SAFETY: We are transferring ownership back to the plugin DLL to free its own memory.
            unsafe {
                (loaded_plugin.destroy_fn)(loaded_plugin.plugin_ptr);
            }

            tracing::debug!("Plugin destroyed: {}", loaded_plugin.metadata.name);

            // Library will be unloaded when Arc drops (if no other references)

            Ok(())
        } else {
            Err(PluginManagerError::PluginNotFound {
                plugin_id: plugin_id.clone(),
            })
        }
    }

    /// Get all loaded plugins.
    pub fn get_plugins(&self) -> Vec<&PluginMetadata> {
        self.plugins.values().map(|p| &p.metadata).collect()
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
    
    // TODO: We should really be keeping track of which plugin owns what editors rather than finding on-the-fly
    /// Find which plugin owns a given editor ID
    pub fn find_plugin_for_editor(&self, editor_id: &EditorId) -> Option<PluginId> {
        for (_id, plugin) in &self.plugins {
            let editors = unsafe {
                let plugin_ref = &*plugin.plugin_ptr;
                plugin_ref.editors()
            };
            
            if editors.iter().any(|e| &e.id == editor_id) {
                return Some(plugin.metadata.id.clone());
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
    /// 1. Tries built-in editors first (no DLL loading)
    /// 2. Falls back to DLL-based plugins if no built-in editor is found
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
                            unimplemented!("Built-in editors handle their own file paths")
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
                        plugin_id: plugin_id.clone(),
                        error: e,
                    });
                }
            }
        }

        // Fall back to DLL-based plugin
        self.create_editor(&plugin_id, &editor_id, file_path.to_path_buf(), window, cx)
    }

    /// Create an editor instance with a specific editor ID.
    pub fn create_editor(
        &mut self,
        plugin_id: &PluginId,
        editor_id: &EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(std::sync::Arc<dyn ui::dock::PanelView>, Box<dyn EditorInstance>), PluginManagerError> {
        let plugin = self
            .plugins
            .get_mut(plugin_id)
            .ok_or_else(|| PluginManagerError::PluginNotFound {
                plugin_id: plugin_id.clone(),
            })?;

        // Validate plugin pointer before use
        if plugin.plugin_ptr.is_null() {
            return Err(PluginManagerError::PluginError {
                plugin_id: plugin_id.clone(),
                error: PluginError::Other {
                    message: "Plugin pointer is null (plugin may have been corrupted)".to_string(),
                },
            });
        }

        // Initialize plugin globals (Theme, etc.) from main app before creating editor
        // This syncs the main app's global state into the plugin's DLL memory space
        unsafe {
            if let Ok(init_fn) = plugin.library.get::<unsafe extern "C" fn(*const std::ffi::c_void)>(b"_plugin_init_globals") {
                // Get Theme pointer from main app's global state
                let theme_ptr = ui::theme::Theme::global(cx) as *const _ as *const std::ffi::c_void;

                // Validate theme pointer before passing to plugin
                if !theme_ptr.is_null() {
                    init_fn(theme_ptr);
                    tracing::debug!("Initialized plugin globals for: {}", plugin_id.as_str());
                } else {
                    tracing::warn!("Theme pointer is null, plugin may not have theme access");
                }
            }
        }

        // Call create_editor via raw pointer
        // SAFETY: Plugin is loaded, pointer validated as non-null above
        //
        // The plugin returns a Weak reference to prevent Arc leaks across DLL boundaries.
        // The plugin maintains the strong Arc internally, and we upgrade the Weak here.
        unsafe {
            let (weak_panel, editor_instance) = (*plugin.plugin_ptr)
                .create_editor(editor_id.clone(), file_path, window, cx, &EditorLogger)
                .map_err(|e| PluginManagerError::PluginError {
                    plugin_id: plugin_id.clone(),
                    error: e,
                })?;

            // Upgrade the Weak to an Arc for the main app to use
            // This is safe because the plugin still holds the strong Arc
            let arc_panel = weak_panel.upgrade().ok_or_else(|| {
                PluginManagerError::PluginError {
                    plugin_id: plugin_id.clone(),
                    error: PluginError::Other {
                        message: "Plugin panel was dropped before use".to_string(),
                    },
                }
            })?;

            tracing::debug!(
                "Created editor (strong: {}, weak: {})",
                std::sync::Arc::strong_count(&arc_panel),
                std::sync::Arc::weak_count(&arc_panel)
            );

            Ok((arc_panel, editor_instance))
        }
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

// When the manager is dropped, properly destroy all plugins
impl Drop for PluginManager {
    fn drop(&mut self) {
        for (plugin_id, loaded_plugin) in self.plugins.drain() {
            // Call on_unload hook
            // SAFETY: Plugin is still valid, about to be destroyed
            unsafe { (*loaded_plugin.plugin_ptr).on_unload() };

            // CRITICAL: Call destroy function to free memory in plugin's heap
            // SAFETY: We are transferring ownership back to the plugin DLL
            unsafe {
                (loaded_plugin.destroy_fn)(loaded_plugin.plugin_ptr);
            }

            tracing::debug!("Destroyed plugin on drop: {}", plugin_id);
        }
    }
}

// ============================================================================
// Plugin Manager Errors
// ============================================================================

/// Errors that can occur in the plugin manager.
#[derive(Debug, Clone)]
pub enum PluginManagerError {
    /// Failed to load dynamic library
    LibraryLoadError { path: PathBuf, message: String },

    /// Required symbol not found in library
    MissingSymbol { symbol: String, message: String },

    /// Plugin version incompatible with engine
    VersionMismatch {
        expected: VersionInfo,
        actual: VersionInfo,
    },

    /// Failed to create plugin instance
    PluginCreationFailed { message: String },

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

    /// Plugin error
    PluginError {
        plugin_id: PluginId,
        error: PluginError,
    },

    /// Failed to create file
    FileCreationError { path: PathBuf, message: String },
}

impl std::fmt::Display for PluginManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryLoadError { path, message } => {
                write!(f, "Failed to load library {:?}: {}", path, message)
            }
            Self::MissingSymbol { symbol, message } => {
                write!(f, "Missing symbol '{}': {}", symbol, message)
            }
            Self::VersionMismatch { expected, actual } => {
                write!(
                    f,
                    "Plugin version mismatch: expected engine v{}.{}.{} with rustc hash {:#x}, got v{}.{}.{} with rustc hash {:#x}. \
                    Plugin must be recompiled with the same Rust compiler version as the engine.",
                    expected.engine_version.0,
                    expected.engine_version.1,
                    expected.engine_version.2,
                    expected.rustc_version_hash,
                    actual.engine_version.0,
                    actual.engine_version.1,
                    actual.engine_version.2,
                    actual.rustc_version_hash,
                )
            }
            Self::PluginCreationFailed { message } => {
                write!(f, "Failed to create plugin: {}", message)
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
            Self::PluginError { plugin_id, error } => {
                write!(f, "Plugin error in {}: {}", plugin_id, error)
            }
            Self::FileCreationError { path, message } => {
                write!(f, "Failed to create file {:?}: {}", path, message)
            }
        }
    }
}

impl std::error::Error for PluginManagerError {}
