//! Asset Operations
//!
//! Handles all file operations (create, update, delete) and maintains index consistency

use anyhow::{Result, Context};
use std::path::PathBuf;
use std::sync::Arc;
use super::TypeAliasIndex;

/// Handles asset file operations
pub struct AssetOperations {
    project_root: PathBuf,
    type_index: Arc<TypeAliasIndex>,
}

impl AssetOperations {
    pub fn new(
        project_root: PathBuf,
        type_index: Arc<TypeAliasIndex>,
    ) -> Self {
        Self {
            project_root,
            type_index,
        }
    }
    
    /// Create a new type alias file
    pub fn create_type_alias(&self, name: &str, content: &str) -> Result<PathBuf> {
        // Validate name is unique
        if !self.type_index.is_name_available(name) {
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
        
        // Register in index
        self.type_index.register(&file_path)?;
        
        Ok(file_path)
    }
    
    /// Update an existing type alias file
    pub fn update_type_alias(&self, file_path: &PathBuf, content: &str) -> Result<()> {
        // Parse to validate before writing
        let asset: ui_types_common::AliasAsset = serde_json::from_str(content)
            .context("Invalid alias JSON")?;
        
        // Validate name is still unique (or same file)
        self.type_index.validate_name(&asset.name, file_path)?;
        
        // Write file
        std::fs::write(file_path, content)
            .context("Failed to write alias file")?;
        
        // Update index
        self.type_index.register(file_path)?;
        
        Ok(())
    }
    
    /// Delete a type alias file
    pub fn delete_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        // Remove from index first
        self.type_index.unregister_by_path(file_path);
        
        // Delete file
        std::fs::remove_file(file_path)
            .context("Failed to delete alias file")?;
        
        Ok(())
    }
    
    /// Register an existing type alias file (for scanning)
    pub fn register_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        self.type_index.register(file_path)
    }
    
    /// Move/rename a type alias file
    pub fn move_type_alias(&self, old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
        // Unregister old
        self.type_index.unregister_by_path(old_path);
        
        // Move file
        std::fs::rename(old_path, new_path)
            .context("Failed to move alias file")?;
        
        // Register new
        self.type_index.register(new_path)?;
        
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
    
    /// Register an asset in the appropriate index
    fn register_asset(&self, file_path: &PathBuf, kind: super::asset_templates::AssetKind) -> Result<()> {
        use super::asset_templates::AssetKind;
        
        match kind {
            AssetKind::TypeAlias => {
                self.type_index.register(file_path)?;
            }
            _ => {
                // Other asset types managed through registry
            }
        }
        
        Ok(())
    }
    
    /// Delete any asset file
    pub fn delete_asset(&self, file_path: &PathBuf) -> Result<()> {
        // Unregister from type index
        self.type_index.unregister_by_path(file_path);
        
        // Delete file
        std::fs::remove_file(file_path)
            .context("Failed to delete asset file")?;
        
        Ok(())
    }
    
    /// Rename/move any asset file
    pub fn move_asset(&self, old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
        // Unregister from indexes
        self.type_index.unregister_by_path(old_path);
        
        // Create parent directory for new path
        if let Some(parent) = new_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        
        // Move file
        std::fs::rename(old_path, new_path)
            .context("Failed to move asset file")?;
        
        // Re-register at new location if it's a type alias
        if let Some(ext) = new_path.extension() {
            let ext_str = ext.to_string_lossy();
            if ext_str.contains("alias") {
                self.type_index.register(new_path)?;
            }
        }
        
        Ok(())
    }
}
