//! Type-specific asset operations (type aliases)
//!
//! Handles create, update, delete, and move operations for type alias files.

use anyhow::{Context, Result};
use plugin_editor_api::FileTypeId;
use std::path::PathBuf;
use std::sync::Arc;
use type_db::TypeDatabase;

/// Type alias operations handler
pub struct TypeOperations {
    project_root: PathBuf,
    type_database: Arc<TypeDatabase>,
}

impl TypeOperations {
    pub fn new(project_root: PathBuf, type_database: Arc<TypeDatabase>) -> Self {
        Self {
            project_root,
            type_database,
        }
    }

    /// Create a new type alias file
    pub fn create_type_alias(&self, name: &str, content: &str) -> Result<PathBuf> {
        // Validate name is unique
        if !self.type_database.get_by_name(name).is_empty() {
            anyhow::bail!("Type alias name '{}' is already in use", name);
        }

        // Determine file path
        let file_path = self
            .project_root
            .join("types")
            .join("aliases")
            .join(format!("{}.alias.json", name));

        // Create parent directories
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file
        std::fs::write(&file_path, content).context("Failed to write alias file")?;

        // Register in type database
        let file_type = FileTypeId::new("alias");
        if let Err(e) = self.type_database.register_with_path(
            name.to_string(),
            file_path.clone(),
            file_type,
            None,
            Some(format!("Type alias: {}", name)),
            None,
        ) {
            tracing::warn!("Failed to register type alias '{}': {:?}", name, e);
        }

        Ok(file_path)
    }

    /// Update an existing type alias file
    pub fn update_type_alias(&self, file_path: &PathBuf, content: &str) -> Result<()> {
        // Parse to validate before writing
        let asset: ui_types_common::AliasAsset =
            serde_json::from_str(content).context("Invalid alias JSON")?;

        // Validate name is still unique (or same file)
        let existing_types = self.type_database.get_by_name(&asset.name);
        for existing in &existing_types {
            if existing.file_path.as_ref() != Some(file_path) {
                anyhow::bail!("Type alias name '{}' is already in use", asset.name);
            }
        }

        // Write file
        std::fs::write(file_path, content).context("Failed to write alias file")?;

        // Update in type database
        let file_type = FileTypeId::new("alias");
        if let Err(e) = self.type_database.register_with_path(
            asset.name.clone(),
            file_path.clone(),
            file_type,
            None,
            Some(format!("Type alias: {}", asset.name)),
            None,
        ) {
            tracing::warn!("Failed to update type alias '{}': {:?}", asset.name, e);
        }

        Ok(())
    }

    /// Delete a type alias file
    pub fn delete_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        // Remove from type database first
        self.type_database.unregister_by_path(file_path);

        // Delete file
        std::fs::remove_file(file_path).context("Failed to delete alias file")?;

        Ok(())
    }

    /// Register an existing type alias file (for scanning)
    pub fn register_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        // Read and parse the file to get the name
        let content = std::fs::read_to_string(file_path).context("Failed to read alias file")?;
        let asset: ui_types_common::AliasAsset =
            serde_json::from_str(&content).context("Invalid alias JSON")?;

        // Register in type database
        let file_type = FileTypeId::new("alias");
        if let Err(e) = self.type_database.register_with_path(
            asset.name.clone(),
            file_path.clone(),
            file_type,
            None,
            Some(format!("Type alias: {}", asset.name)),
            None,
        ) {
            tracing::warn!("Failed to register type alias '{}': {:?}", asset.name, e);
        }

        Ok(())
    }

    /// Move/rename a type alias file
    pub fn move_type_alias(&self, old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
        // Unregister old
        self.type_database.unregister_by_path(old_path);

        // Move file
        std::fs::rename(old_path, new_path).context("Failed to move alias file")?;

        // Read and register at new location
        let content = std::fs::read_to_string(new_path).context("Failed to read alias file")?;
        let asset: ui_types_common::AliasAsset =
            serde_json::from_str(&content).context("Invalid alias JSON")?;

        let file_type = FileTypeId::new("alias");
        if let Err(e) = self.type_database.register_with_path(
            asset.name.clone(),
            new_path.clone(),
            file_type,
            None,
            Some(format!("Type alias: {}", asset.name)),
            None,
        ) {
            tracing::warn!(
                "Failed to register renamed type alias '{}': {:?}",
                asset.name,
                e
            );
        }

        Ok(())
    }
}
