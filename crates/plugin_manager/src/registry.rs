//! Registries for file types and editors.
//!
//! These registries maintain the mapping between file types, editors, and plugins.

use plugin_editor_api::*;
use std::collections::HashMap;
use std::path::Path;

// ============================================================================
// File Type Registry
// ============================================================================

/// Registry for all file types provided by plugins.
pub struct FileTypeRegistry {
    /// All registered file types, indexed by FileTypeId
    file_types: HashMap<FileTypeId, FileTypeDefinition>,

    /// Map from file extension to FileTypeId
    /// For folder-based files, this is the folder extension
    extension_to_type: HashMap<String, FileTypeId>,

    /// Map from FileTypeId to PluginId (which plugin provides this type)
    type_to_plugin: HashMap<FileTypeId, PluginId>,
}

impl FileTypeRegistry {
    pub fn new() -> Self {
        Self {
            file_types: HashMap::new(),
            extension_to_type: HashMap::new(),
            type_to_plugin: HashMap::new(),
        }
    }

    /// Register a file type from a plugin.
    pub fn register(&mut self, file_type: FileTypeDefinition, plugin_id: PluginId) {
        let file_type_id = file_type.id.clone();
        let extension = file_type.extension.clone();

        // Store the file type
        self.file_types.insert(file_type_id.clone(), file_type);

        // Map extension to type
        self.extension_to_type
            .insert(extension, file_type_id.clone());

        // Map type to plugin
        self.type_to_plugin.insert(file_type_id, plugin_id);
    }

    /// Unregister all file types from a plugin.
    pub fn unregister_by_plugin(&mut self, plugin_id: &PluginId) {
        // Find all file types from this plugin
        let file_type_ids: Vec<FileTypeId> = self
            .type_to_plugin
            .iter()
            .filter(|(_, pid)| *pid == plugin_id)
            .map(|(ftid, _)| ftid.clone())
            .collect();

        // Remove them
        for file_type_id in file_type_ids {
            self.unregister(&file_type_id);
        }
    }

    /// Unregister a specific file type.
    pub fn unregister(&mut self, file_type_id: &FileTypeId) {
        if let Some(file_type) = self.file_types.remove(file_type_id) {
            self.extension_to_type.remove(&file_type.extension);
            self.type_to_plugin.remove(file_type_id);
        }
    }

    /// Get a file type by ID.
    pub fn get_file_type(&self, file_type_id: &FileTypeId) -> Option<&FileTypeDefinition> {
        self.file_types.get(file_type_id)
    }

    /// Get all registered file types.
    pub fn get_all_file_types(&self) -> Vec<&FileTypeDefinition> {
        self.file_types.values().collect()
    }

    /// Get the file type for a path.
    ///
    /// This checks:
    /// 1. If the path is a folder with an extension (folder-based file)
    /// 2. If the path is a regular file with an extension
    pub fn get_file_type_for_path(&self, path: &Path) -> Option<FileTypeId> {
        // Check if this is a folder with an extension
        if path.is_dir() {
            if let Some(folder_name) = path.file_name().and_then(|s| s.to_str()) {
                // Check if the folder name has an extension
                if let Some(dot_pos) = folder_name.rfind('.') {
                    let ext = &folder_name[dot_pos + 1..];
                    if let Some(file_type_id) = self.extension_to_type.get(ext) {
                        // Verify this is a folder-based file type
                        if let Some(file_type) = self.file_types.get(file_type_id) {
                            if matches!(file_type.structure, FileStructure::FolderBased { .. }) {
                                return Some(file_type_id.clone());
                            }
                        }
                    }
                }
            }

            // If no extension found, check for marker files
            // This handles folders like "ExampleClass/" that contain "graph_save.json"
            for (file_type_id, file_type) in &self.file_types {
                if let FileStructure::FolderBased { marker_file, .. } = &file_type.structure {
                    let marker_path = path.join(marker_file);
                    if marker_path.exists() {
                        return Some(file_type_id.clone());
                    }
                }
            }
        }

        // Check if this is a regular file
        if let Some(extension) = path.extension().and_then(|s| s.to_str()) {
            if let Some(file_type_id) = self.extension_to_type.get(extension) {
                return Some(file_type_id.clone());
            }
        }

        // Check for compound extensions like .struct.json
        if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
            // Try to find the longest matching extension
            let parts: Vec<&str> = file_name.split('.').collect();
            for i in 1..parts.len() {
                let compound_ext = parts[i..].join(".");
                if let Some(file_type_id) = self.extension_to_type.get(&compound_ext) {
                    return Some(file_type_id.clone());
                }
            }
        }

        None
    }

    /// Get the plugin that provides a file type.
    pub fn get_plugin_for_file_type(&self, file_type_id: &FileTypeId) -> Option<&PluginId> {
        self.type_to_plugin.get(file_type_id)
    }

    /// Check if a path matches a folder-based file type.
    ///
    /// Returns the file type ID if the path is a folder containing the marker file.
    pub fn check_folder_based_file(&self, path: &Path) -> Option<FileTypeId> {
        if !path.is_dir() {
            return None;
        }

        // First check if the folder has an extension that matches a folder-based type
        let folder_type = self.get_file_type_for_path(path)?;

        // Verify the marker file exists
        if let Some(file_type) = self.file_types.get(&folder_type) {
            if let FileStructure::FolderBased { marker_file, .. } = &file_type.structure {
                let marker_path = path.join(marker_file);
                if marker_path.exists() {
                    return Some(folder_type);
                }
            }
        }

        None
    }
}

impl Default for FileTypeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Editor Registry
// ============================================================================

/// Registry for all editors provided by plugins.
pub struct EditorRegistry {
    /// All registered editors, indexed by EditorId
    editors: HashMap<EditorId, EditorMetadata>,

    /// Map from EditorId to PluginId (which plugin provides this editor)
    editor_to_plugin: HashMap<EditorId, PluginId>,

    /// Map from FileTypeId to a list of EditorIds that can open it
    file_type_to_editors: HashMap<FileTypeId, Vec<EditorId>>,
}

impl EditorRegistry {
    pub fn new() -> Self {
        Self {
            editors: HashMap::new(),
            editor_to_plugin: HashMap::new(),
            file_type_to_editors: HashMap::new(),
        }
    }

    /// Register an editor from a plugin.
    pub fn register(&mut self, editor: EditorMetadata, plugin_id: PluginId) {
        let editor_id = editor.id.clone();

        // Register this editor for all its supported file types
        for file_type_id in &editor.supported_file_types {
            self.file_type_to_editors
                .entry(file_type_id.clone())
                .or_insert_with(Vec::new)
                .push(editor_id.clone());
        }

        // Store the editor
        self.editors.insert(editor_id.clone(), editor);

        // Map editor to plugin
        self.editor_to_plugin.insert(editor_id, plugin_id);
    }

    /// Unregister all editors from a plugin.
    pub fn unregister_by_plugin(&mut self, plugin_id: &PluginId) {
        // Find all editors from this plugin
        let editor_ids: Vec<EditorId> = self
            .editor_to_plugin
            .iter()
            .filter(|(_, pid)| *pid == plugin_id)
            .map(|(eid, _)| eid.clone())
            .collect();

        // Remove them
        for editor_id in editor_ids {
            self.unregister(&editor_id);
        }
    }

    /// Unregister a specific editor.
    pub fn unregister(&mut self, editor_id: &EditorId) {
        if let Some(editor) = self.editors.remove(editor_id) {
            // Remove from file type mappings
            for file_type_id in &editor.supported_file_types {
                if let Some(editors) = self.file_type_to_editors.get_mut(file_type_id) {
                    editors.retain(|eid| eid != editor_id);
                    if editors.is_empty() {
                        self.file_type_to_editors.remove(file_type_id);
                    }
                }
            }

            // Remove from plugin mapping
            self.editor_to_plugin.remove(editor_id);
        }
    }

    /// Get an editor by ID.
    pub fn get_editor(&self, editor_id: &EditorId) -> Option<&EditorMetadata> {
        self.editors.get(editor_id)
    }

    /// Get all registered editors.
    pub fn get_all_editors(&self) -> Vec<&EditorMetadata> {
        self.editors.values().collect()
    }

    /// Get the first editor that can open a file type.
    ///
    /// If multiple editors support the same file type, this returns the first one.
    /// In the future, we could add logic for user preferences or editor priorities.
    pub fn get_editor_for_file_type(&self, file_type_id: &FileTypeId) -> Option<EditorId> {
        self.file_type_to_editors
            .get(file_type_id)
            .and_then(|editors| editors.first())
            .cloned()
    }

    /// Get all editors that can open a file type.
    pub fn get_editors_for_file_type(&self, file_type_id: &FileTypeId) -> Vec<EditorId> {
        self.file_type_to_editors
            .get(file_type_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Get the plugin that provides an editor.
    pub fn get_plugin_for_editor(&self, editor_id: &EditorId) -> Option<&PluginId> {
        self.editor_to_plugin.get(editor_id)
    }
}

impl Default for EditorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_type_registry() {
        let mut registry = FileTypeRegistry::new();

        let plugin_id = PluginId::new("test.plugin");
        let file_type = standalone_file_type(
            "test-file",
            "test",
            "Test File",
            ui::IconName::Code,
            gpui::rgb(0x00BCD4).into(),
            serde_json::json!({}),
        );

        registry.register(file_type, plugin_id.clone());

        assert!(registry.get_file_type(&FileTypeId::new("test-file")).is_some());
        assert_eq!(
            registry.get_plugin_for_file_type(&FileTypeId::new("test-file")),
            Some(&plugin_id)
        );
    }

    #[test]
    fn test_editor_registry() {
        let mut registry = EditorRegistry::new();

        let plugin_id = PluginId::new("test.plugin");
        let editor = EditorMetadata {
            id: EditorId::new("test-editor"),
            display_name: "Test Editor".to_string(),
            supported_file_types: vec![FileTypeId::new("test-file")],
        };

        registry.register(editor, plugin_id.clone());

        assert!(registry.get_editor(&EditorId::new("test-editor")).is_some());
        assert_eq!(
            registry.get_editor_for_file_type(&FileTypeId::new("test-file")),
            Some(EditorId::new("test-editor"))
        );
    }
}
