//! Creating editors and files: the path from a file on disk to a panel.
//!
//! Entry points: [`PluginManager::create_editor_for_file`] (resolves file type,
//! editor and owning plugin) and [`PluginManager::create_new_file`]. Built-in
//! editors are created directly; plugin editors go through the factory the
//! plugin registered when it was installed.

use crate::{EditorContext, PluginManager, PluginManagerError};
use plugin_editor_api::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dock::PanelView;

impl PluginManager {
    /// Create an editor instance for a file.
    ///
    /// This will:
    /// 1. Determine the file type from the path
    /// 2. Find an editor that supports that file type
    /// 3. Create an editor instance using the appropriate plugin or built-in editor
    ///
    /// # Returns
    ///
    /// Returns `Arc<dyn PanelView>` which the caller can use directly.
    ///
    /// # Safety
    ///
    /// Because plugins are never unloaded, the returned `Arc` is safe to hold
    /// indefinitely. The drop glue and vtable will remain valid for the process
    /// lifetime.
    ///
    /// # Memory Management
    ///
    /// The returned `Arc` uses normal Rust reference counting. When all references
    /// are dropped, the editor's `Drop` implementation will be called (which is safe
    /// because the plugin code is still loaded).
    ///
    /// **Avoid Arc cycles**: Use `Weak<T>` for back-references to prevent memory leaks.
    pub fn create_editor_for_file(
        &mut self,
        file_path: &Path,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginManagerError> {
        let project_root = self.project_root.clone();
        self.create_editor_for_file_with_project_root(file_path, project_root, window, cx)
    }

    /// Create an editor for a file while giving it an explicit tree root.
    /// This is used by engine source browsing, where the editor must browse the
    /// engine workspace even when a game project is currently open.
    pub fn create_editor_for_file_with_project_root(
        &mut self,
        file_path: &Path,
        project_root: Option<PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginManagerError> {
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

        // Check if this is a built-in editor
        if plugin_id.as_str() == "builtin" {
            // Create editor context with project root
            let editor_context =
                EditorContext::new(project_root).with_services(self.editor_services());

            // Create the editor directly using the provider
            return self
                .builtin_registry
                .create_editor(
                    &editor_id,
                    file_path.to_path_buf(),
                    &editor_context,
                    window,
                    cx,
                )
                .map(|panel| self.decorate_editor_panel_for_path(panel, file_path))
                .map_err(|e| PluginManagerError::PluginError {
                    plugin_id,
                    error: e,
                });
        }

        // Fall back to DLL-based plugin
        self.create_editor(&plugin_id, &editor_id, file_path.to_path_buf(), window, cx)
    }

    /// Create an editor instance with a specific editor ID.
    ///
    /// # Returns
    ///
    /// Returns `Arc<dyn PanelView>` which is safe to hold indefinitely because
    /// the plugin is never unloaded.
    pub fn create_editor(
        &mut self,
        plugin_id: &PluginId,
        editor_id: &EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginManagerError> {
        let file_path_for_decoration = file_path.clone();
        let settings = self.snapshot_for_editor();

        let plugin =
            self.plugins
                .get_mut(plugin_id)
                .ok_or_else(|| PluginManagerError::PluginNotFound {
                    plugin_id: plugin_id.clone(),
                })?;

        // Initialize plugin globals (Theme, etc.) from main app before creating editor
        // This syncs the main app's global state into the plugin's DLL memory space
        unsafe {
            // SAFETY: We're calling an optional init function exported by the plugin.
            // This function updates the plugin's copy of global state (Theme, etc.)
            // to match the engine's current state.
            if let Ok(init_fn) = plugin
                .library
                .get::<unsafe extern "C" fn(*const std::ffi::c_void)>(b"_plugin_init_globals")
            {
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

        // Look up the editor factory and create the editor
        // SAFETY: The returned Arc<dyn PanelView> is safe because:
        // 1. The vtable lives in the plugin's .rodata section (never unmapped)
        // 2. The drop glue lives in the plugin's .text section (never unmapped)
        // 3. We can safely share the Arc across the boundary
        let factory = plugin.editor_factories.get(editor_id).ok_or_else(|| {
            PluginManagerError::EditorNotFound {
                editor_id: editor_id.clone(),
            }
        })?;

        (factory.create)(file_path, &settings, window, cx)
            .map(|panel| self.decorate_editor_panel_for_path(panel, &file_path_for_decoration))
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

    /// Get the file type definition for a file path.
    ///
    /// This looks up the file type ID from the path and returns the full FileTypeDefinition
    /// which includes the icon and other metadata useful for UI display.
    pub fn get_file_type_for_path(
        &self,
        path: &Path,
    ) -> Option<&plugin_editor_api::FileTypeDefinition> {
        self.file_type_registry
            .get_file_type_for_path(path)
            .and_then(|id| self.file_type_registry.get_file_type(&id))
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
                let content =
                    serde_json::to_string_pretty(&file_type.default_content).map_err(|e| {
                        PluginManagerError::FileCreationError {
                            path: path.to_path_buf(),
                            message: e.to_string(),
                        }
                    })?;

                engine_fs::virtual_fs::write_file(path, content.as_bytes()).map_err(|e| {
                    PluginManagerError::FileCreationError {
                        path: path.to_path_buf(),
                        message: e.to_string(),
                    }
                })?;
            }

            FileStructure::FolderBased {
                marker_file,
                template_structure,
            } => {
                // Create the folder
                engine_fs::virtual_fs::create_dir_all(path).map_err(|e| {
                    PluginManagerError::FileCreationError {
                        path: path.to_path_buf(),
                        message: e.to_string(),
                    }
                })?;

                // Create the marker file with default content
                let marker_path = path.join(marker_file);
                let content =
                    serde_json::to_string_pretty(&file_type.default_content).map_err(|e| {
                        PluginManagerError::FileCreationError {
                            path: marker_path.clone(),
                            message: e.to_string(),
                        }
                    })?;

                engine_fs::virtual_fs::write_file(&marker_path, content.as_bytes()).map_err(
                    |e| PluginManagerError::FileCreationError {
                        path: marker_path,
                        message: e.to_string(),
                    },
                )?;

                // Create template structure
                for template in template_structure {
                    match template {
                        PathTemplate::File {
                            path: rel_path,
                            content,
                        } => {
                            let file_path = path.join(rel_path);
                            if let Some(parent) = file_path.parent() {
                                let _ = engine_fs::virtual_fs::create_dir_all(parent);
                            }
                            engine_fs::virtual_fs::write_file(&file_path, content.as_bytes())
                                .map_err(|e| PluginManagerError::FileCreationError {
                                    path: file_path,
                                    message: e.to_string(),
                                })?;
                        }
                        PathTemplate::Folder { path: rel_path } => {
                            let folder_path = path.join(rel_path);
                            engine_fs::virtual_fs::create_dir_all(&folder_path).map_err(|e| {
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
