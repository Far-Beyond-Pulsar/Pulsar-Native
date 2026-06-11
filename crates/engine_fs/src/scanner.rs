//! Project scanning and indexing
//!
//! Handles scanning the project directory and registering assets in the asset index,
//! and registering user-defined type aliases in the user type registry.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

use crate::asset_index::AssetIndex;
use crate::user_types::UserTypeRegistry;

/// Project scanner for indexing assets
pub struct ProjectScanner {
    project_root: PathBuf,
    asset_index: Arc<AssetIndex>,
    user_types: Arc<UserTypeRegistry>,
}

impl ProjectScanner {
    pub fn new(
        project_root: PathBuf,
        asset_index: Arc<AssetIndex>,
        user_types: Arc<UserTypeRegistry>,
    ) -> Self {
        Self {
            project_root,
            asset_index,
            user_types,
        }
    }

    /// Scan the entire project and build the asset index and user type registry
    pub fn scan_project(&mut self) -> Result<()> {
        use walkdir::WalkDir;

        // Clear existing indexes
        self.asset_index.clear();
        self.user_types.clear();

        // Walk the project directory
        for entry in WalkDir::new(&self.project_root)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            // Skip hidden files and target directory
            if path.components().any(|c| {
                c.as_os_str().to_string_lossy().starts_with('.') || c.as_os_str() == "target"
            }) {
                continue;
            }

            // Register based on file extension
            if path.is_file() {
                self.register_asset(path.to_path_buf())?;
            }
        }

        Ok(())
    }

    /// Register a single asset file using the plugin registry
    fn register_asset(&self, path: PathBuf) -> Result<()> {
        // Use the global registry to determine file type
        if let Some(plugin_manager) = plugin_manager::global() {
            if let Ok(pm) = plugin_manager.read() {
                if let Some(file_type_id) = pm.file_type_registry().get_file_type_for_path(&path) {
                    if let Some(file_type_def) =
                        pm.file_type_registry().get_file_type(&file_type_id)
                    {
                        // Get the type name from the parent folder or file stem
                        let type_name = path
                            .parent()
                            .and_then(|p| p.file_name())
                            .and_then(|n| n.to_str())
                            .or_else(|| path.file_stem().and_then(|n| n.to_str()))
                            .unwrap_or("unknown")
                            .to_string();

                        // Register with FileTypeId from registry
                        if let Err(e) = self.asset_index.register_with_path(
                            type_name.clone(),
                            path.clone(),
                            file_type_id.clone(),
                            None,
                            Some(format!("{}: {}", file_type_def.display_name, type_name)),
                        ) {
                            tracing::warn!("Failed to register asset '{}': {:?}", type_name, e);
                        }

                        // Additionally register user-defined type aliases in the
                        // dynamic type registry
                        if file_type_id.as_str() == "alias" {
                            if let Err(e) = self.user_types.register_alias_file(&path) {
                                tracing::warn!(
                                    "Failed to register type alias at {:?}: {:?}",
                                    path,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
