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

use libloading::{Library, Symbol};
use plugin_editor_api::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod registry;
pub use registry::{EditorRegistry, FileTypeRegistry};

// ============================================================================
// Plugin Container
// ============================================================================

/// A loaded plugin with its library handle.
struct LoadedPlugin {
    /// The plugin instance
    plugin: Box<dyn EditorPlugin>,
    /// The dynamic library handle (must be kept alive)
    #[allow(dead_code)]
    library: Arc<Library>,
    /// Metadata for quick access
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
pub struct PluginManager {
    /// All loaded plugins, indexed by plugin ID
    plugins: HashMap<PluginId, LoadedPlugin>,

    /// Registry of all file types
    file_type_registry: FileTypeRegistry,

    /// Registry of all editors
    editor_registry: EditorRegistry,

    /// The version info for this engine build
    engine_version: VersionInfo,
}

impl PluginManager {
    /// Create a new plugin manager.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            file_type_registry: FileTypeRegistry::new(),
            editor_registry: EditorRegistry::new(),
            engine_version: VersionInfo::current(),
        }
    }

    /// Load all plugins from a directory.
    ///
    /// This will scan the directory for dynamic libraries (.dll on Windows,
    /// .so on Linux, .dylib on macOS) and attempt to load each one as a plugin.
    ///
    /// Plugins that fail version checks or loading will be logged but won't
    /// prevent other plugins from loading.
    pub fn load_plugins_from_dir(&mut self, dir: impl AsRef<Path>) -> Result<(), PluginManagerError> {
        let dir = dir.as_ref();

        if !dir.exists() {
            log::warn!("Plugin directory does not exist: {:?}", dir);
            return Ok(());
        }

        log::info!("Loading plugins from: {:?}", dir);

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
            match self.load_plugin(path) {
                Ok(plugin_id) => {
                    log::info!("Successfully loaded plugin: {}", plugin_id);
                }
                Err(e) => {
                    log::error!("Failed to load plugin from {:?}: {}", path, e);
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
    pub fn load_plugin(&mut self, path: impl AsRef<Path>) -> Result<PluginId, PluginManagerError> {
        let path = path.as_ref();

        log::debug!("Loading plugin from: {:?}", path);

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
        if !self.engine_version.is_compatible(&plugin_version) {
            return Err(PluginManagerError::VersionMismatch {
                expected: self.engine_version,
                actual: plugin_version,
            });
        }

        // Get the plugin constructor
        let create_fn: Symbol<PluginCreate> = unsafe {
            library
                .get(b"_plugin_create")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_create".to_string(),
                    message: e.to_string(),
                })?
        };

        // Create the plugin instance
        let mut plugin = unsafe {
            let raw_plugin = create_fn();
            if raw_plugin.is_null() {
                return Err(PluginManagerError::PluginCreationFailed {
                    message: "Plugin constructor returned null".to_string(),
                });
            }
            Box::from_raw(raw_plugin)
        };

        // Get plugin metadata
        let metadata = plugin.metadata();
        let plugin_id = metadata.id.clone();

        log::info!(
            "Loaded plugin: {} v{} by {}",
            metadata.name,
            metadata.version,
            metadata.author
        );

        // Call on_load hook
        plugin.on_load();

        // Register file types
        for file_type in plugin.file_types() {
            log::debug!(
                "  Registering file type: {} (.{})",
                file_type.display_name,
                file_type.extension
            );
            self.file_type_registry.register(file_type, plugin_id.clone());
        }

        // Register editors
        for editor in plugin.editors() {
            log::debug!("  Registering editor: {}", editor.display_name);
            self.editor_registry.register(editor, plugin_id.clone());
        }

        // Store the plugin
        let loaded_plugin = LoadedPlugin {
            plugin,
            library,
            metadata: metadata.clone(),
        };

        self.plugins.insert(plugin_id.clone(), loaded_plugin);

        Ok(plugin_id)
    }

    /// Unload a plugin by ID.
    ///
    /// This will call the plugin's `on_unload` hook and remove all registered
    /// file types and editors.
    pub fn unload_plugin(&mut self, plugin_id: &PluginId) -> Result<(), PluginManagerError> {
        if let Some(mut loaded_plugin) = self.plugins.remove(plugin_id) {
            // Call on_unload hook
            loaded_plugin.plugin.on_unload();

            // Remove file types
            self.file_type_registry.unregister_by_plugin(plugin_id);

            // Remove editors
            self.editor_registry.unregister_by_plugin(plugin_id);

            log::info!("Unloaded plugin: {}", loaded_plugin.metadata.name);

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

    /// Create an editor instance for a file.
    ///
    /// This will:
    /// 1. Determine the file type from the path
    /// 2. Find an editor that supports that file type
    /// 3. Create an editor instance using the appropriate plugin
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

        // Create the editor instance
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

        plugin
            .plugin
            .create_editor(editor_id.clone(), file_path, window, cx)
            .map_err(|e| PluginManagerError::PluginError {
                plugin_id: plugin_id.clone(),
                error: e,
            })
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

// When the manager is dropped, unload all plugins
impl Drop for PluginManager {
    fn drop(&mut self) {
        for (plugin_id, mut loaded_plugin) in self.plugins.drain() {
            loaded_plugin.plugin.on_unload();
            log::debug!("Unloaded plugin on drop: {}", plugin_id);
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
                    "Version mismatch: expected {:?}, got {:?}",
                    expected, actual
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
