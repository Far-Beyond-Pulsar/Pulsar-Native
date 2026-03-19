//! General asset operations
//!
//! Handles create, delete, and move operations for all asset types.

use anyhow::{Result, Context};
use std::path::PathBuf;
use std::sync::Arc;
use type_db::TypeDatabase;
use plugin_editor_api::FileTypeId;

use crate::templates::AssetKind;

/// General asset operations handler
pub struct GeneralOperations {
    project_root: PathBuf,
    type_database: Arc<TypeDatabase>,
}

impl GeneralOperations {
    pub fn new(project_root: PathBuf, type_database: Arc<TypeDatabase>) -> Self {
        Self {
            project_root,
            type_database,
        }
    }

    /// Create a new asset of any kind
    pub fn create_asset(&self, kind: AssetKind, name: &str, custom_dir: Option<&str>) -> Result<PathBuf> {
        // Generate template content
        let content = kind.generate_template(name);

        // Determine file path
        let dir = custom_dir.unwrap_or(kind.default_directory());
        let extension = kind.extension();
        let file_name = if extension.contains('.') {
            format!("{}.{}", name, extension)
        } else {
            format!("{}.{}", name, extension)
        };

        let file_path = self.project_root
            .join(dir)
            .join(&file_name);

        // Create parent directories
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Special handling for data tables
        if kind == AssetKind::DataTable {
            // Create empty SQLite database
            use std::process::Command;
            Command::new("sqlite3")
                .arg(&file_path)
                .arg("VACUUM;")
                .output()
                .context("Failed to create SQLite database")?;
        } else {
            // Write template content
            std::fs::write(&file_path, content)
                .context("Failed to write asset file")?;
        }

        // Register in appropriate index
        self.register_asset(&file_path, kind)?;

        Ok(file_path)
    }

    /// Register an asset in the type database
    fn register_asset(&self, file_path: &PathBuf, kind: AssetKind) -> Result<()> {
        // Get the file name to use as the type name
        let name = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let file_type_id = match kind {
            AssetKind::TypeAlias => FileTypeId::new("alias"),
            AssetKind::Struct => FileTypeId::new("struct"),
            AssetKind::Enum => FileTypeId::new("enum"),
            AssetKind::Trait => FileTypeId::new("trait"),
            _ => return Ok(()), // Other asset types don't need indexing yet
        };

        if let Err(e) = self.type_database.register_with_path(
            name.clone(),
            file_path.clone(),
            file_type_id.clone(),
            None,
            Some(format!("{:?}: {}", file_type_id, name)),
            None,
        ) {
            tracing::warn!("Failed to register type '{}': {:?}", name, e);
        }

        Ok(())
    }

    /// Delete any asset file
    pub fn delete_asset(&self, file_path: &PathBuf) -> Result<()> {
        // Unregister from type database
        self.type_database.unregister_by_path(file_path);

        // Delete file
        std::fs::remove_file(file_path)
            .context("Failed to delete asset file")?;

        Ok(())
    }

    /// Rename/move any asset file
    pub fn move_asset(&self, old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
        // Unregister from type database
        self.type_database.unregister_by_path(old_path);

        // Create parent directory for new path
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Move file
        std::fs::rename(old_path, new_path)
            .context("Failed to move asset file")?;

        // Re-register at new location using registry
        if let Some(plugin_manager) = plugin_manager::global() {
            if let Ok(pm) = plugin_manager.read() {
                if let Some(file_type_id) = pm.file_type_registry().get_file_type_for_path(new_path) {
                    if let Some(file_type_def) = pm.file_type_registry().get_file_type(&file_type_id) {
                        let name = new_path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("unknown")
                            .to_string();

                        if let Err(e) = self.type_database.register_with_path(
                            name.clone(),
                            new_path.clone(),
                            file_type_id,
                            None,
                            Some(format!("{}: {}", file_type_def.display_name, name)),
                            None,
                        ) {
                            tracing::warn!("Failed to register renamed type '{}': {:?}", name, e);
                        }
                    }
                }
            }
        }

        Ok(())
    }
}
