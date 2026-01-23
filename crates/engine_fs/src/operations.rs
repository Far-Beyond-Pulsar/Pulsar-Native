//! Asset Operations
//!
//! Handles all file operations (create, update, delete) and maintains index consistency

use anyhow::{Result, Context};
use std::path::PathBuf;
use std::sync::Arc;
use type_db::{TypeDatabase, TypeKind};

/// Handles asset file operations
pub struct AssetOperations {
    project_root: PathBuf,
    type_database: Arc<TypeDatabase>,
}

impl AssetOperations {
    pub fn new(
        project_root: PathBuf,
        type_database: Arc<TypeDatabase>,
    ) -> Self {
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
        let file_path = self.project_root
            .join("types")
            .join("aliases")
            .join(format!("{}.alias.json", name));

        // Create parent directories
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write file
        std::fs::write(&file_path, content)
            .context("Failed to write alias file")?;

        // Register in type database
        if let Err(e) = self.type_database.register_with_path(
            name.to_string(),
            file_path.clone(),
            TypeKind::Alias,
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
        let asset: ui_types_common::AliasAsset = serde_json::from_str(content)
            .context("Invalid alias JSON")?;

        // Validate name is still unique (or same file)
        let existing_types = self.type_database.get_by_name(&asset.name);
        for existing in &existing_types {
            if existing.file_path.as_ref() != Some(file_path) {
                anyhow::bail!("Type alias name '{}' is already in use", asset.name);
            }
        }

        // Write file
        std::fs::write(file_path, content)
            .context("Failed to write alias file")?;

        // Update in type database
        if let Err(e) = self.type_database.register_with_path(
            asset.name.clone(),
            file_path.clone(),
            TypeKind::Alias,
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
        std::fs::remove_file(file_path)
            .context("Failed to delete alias file")?;

        Ok(())
    }

    /// Register an existing type alias file (for scanning)
    pub fn register_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        // Read and parse the file to get the name
        let content = std::fs::read_to_string(file_path)
            .context("Failed to read alias file")?;
        let asset: ui_types_common::AliasAsset = serde_json::from_str(&content)
            .context("Invalid alias JSON")?;

        // Register in type database
        if let Err(e) = self.type_database.register_with_path(
            asset.name.clone(),
            file_path.clone(),
            TypeKind::Alias,
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
        std::fs::rename(old_path, new_path)
            .context("Failed to move alias file")?;

        // Read and register at new location
        let content = std::fs::read_to_string(new_path)
            .context("Failed to read alias file")?;
        let asset: ui_types_common::AliasAsset = serde_json::from_str(&content)
            .context("Invalid alias JSON")?;

        if let Err(e) = self.type_database.register_with_path(
            asset.name.clone(),
            new_path.clone(),
            TypeKind::Alias,
            None,
            Some(format!("Type alias: {}", asset.name)),
            None,
        ) {
            tracing::warn!("Failed to register renamed type alias '{}': {:?}", asset.name, e);
        }

        Ok(())
    }
    
    /// Create a new asset of any kind
    pub fn create_asset(&self, kind: super::asset_templates::AssetKind, name: &str, custom_dir: Option<&str>) -> Result<PathBuf> {
        use super::asset_templates::AssetKind;
        
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
    fn register_asset(&self, file_path: &PathBuf, kind: super::asset_templates::AssetKind) -> Result<()> {
        use super::asset_templates::AssetKind;

        // Get the file name to use as the type name
        let name = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let type_kind = match kind {
            AssetKind::TypeAlias => TypeKind::Alias,
            AssetKind::Struct => TypeKind::Struct,
            AssetKind::Enum => TypeKind::Enum,
            AssetKind::Trait => TypeKind::Trait,
            _ => return Ok(()), // Other asset types don't need indexing yet
        };

        if let Err(e) = self.type_database.register_with_path(
            name.clone(),
            file_path.clone(),
            type_kind,
            None,
            Some(format!("{:?}: {}", type_kind, name)),
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

        // Re-register at new location
        // Auto-detect type from extension
        if let Some(ext) = new_path.extension() {
            let ext_str = ext.to_string_lossy();
            let name = new_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            let type_kind = if ext_str.contains("alias") {
                Some(TypeKind::Alias)
            } else if ext_str.contains("struct") {
                Some(TypeKind::Struct)
            } else if ext_str.contains("enum") {
                Some(TypeKind::Enum)
            } else if ext_str.contains("trait") {
                Some(TypeKind::Trait)
            } else {
                None
            };

            if let Some(kind) = type_kind {
                if let Err(e) = self.type_database.register_with_path(
                    name.clone(),
                    new_path.clone(),
                    kind,
                    None,
                    Some(format!("{:?}: {}", kind, name)),
                    None,
                ) {
                    tracing::warn!("Failed to register renamed type '{}': {:?}", name, e);
                }
            }
        }

        Ok(())
    }
}
