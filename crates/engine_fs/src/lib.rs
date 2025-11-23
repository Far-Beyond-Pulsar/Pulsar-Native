//! Engine Filesystem Layer
//!
//! Centralized asset management and indexing system for Pulsar Engine.
//! Handles all file operations and maintains up-to-date indexes for quick lookups.

pub mod asset_registry;
pub mod type_index;
pub mod watchers;
pub mod operations;
pub mod asset_templates;

pub use asset_registry::AssetRegistry;
pub use type_index::{TypeAliasIndex, TypeAliasSignature};
pub use operations::AssetOperations;
pub use asset_templates::{AssetKind, AssetCategory};

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

/// The main engine filesystem manager
/// Coordinates all asset operations and maintains indexes
pub struct EngineFs {
    project_root: PathBuf,
    registry: Arc<AssetRegistry>,
    type_index: Arc<TypeAliasIndex>,
    operations: AssetOperations,
}

impl EngineFs {
    /// Create a new EngineFs instance for a project
    pub fn new(project_root: PathBuf) -> Result<Self> {
        let registry = Arc::new(AssetRegistry::new());
        let type_index = Arc::new(TypeAliasIndex::new());
        let operations = AssetOperations::new(
            project_root.clone(),
            registry.clone(),
            type_index.clone(),
        );

        let mut fs = Self {
            project_root,
            registry,
            type_index,
            operations,
        };

        // Initial scan of the project
        fs.scan_project()?;

        Ok(fs)
    }

    /// Get the project root path
    pub fn project_root(&self) -> &PathBuf {
        &self.project_root
    }

    /// Get the asset registry
    pub fn registry(&self) -> &Arc<AssetRegistry> {
        &self.registry
    }

    /// Get the type alias index
    pub fn type_index(&self) -> &Arc<TypeAliasIndex> {
        &self.type_index
    }

    /// Get asset operations handler
    pub fn operations(&self) -> &AssetOperations {
        &self.operations
    }

    /// Scan the entire project and build indexes
    pub fn scan_project(&mut self) -> Result<()> {
        use walkdir::WalkDir;

        // Clear existing indexes
        self.registry.clear();
        self.type_index.clear();

        // Walk the project directory
        for entry in WalkDir::new(&self.project_root)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            
            // Skip hidden files and target directory
            if path.components().any(|c| {
                c.as_os_str().to_string_lossy().starts_with('.')
                    || c.as_os_str() == "target"
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

    /// Register a single asset file
    fn register_asset(&self, path: PathBuf) -> Result<()> {
        if let Some(extension) = path.extension() {
            match extension.to_string_lossy().as_ref() {
                "alias" | "json" if path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains("alias"))
                    .unwrap_or(false) => 
                {
                    // Type alias file
                    self.operations.register_type_alias(&path)?;
                }
                "struct" => {
                    // Struct definition
                    self.registry.register_struct(&path)?;
                }
                "enum" => {
                    // Enum definition  
                    self.registry.register_enum(&path)?;
                }
                "trait" => {
                    // Trait definition
                    self.registry.register_trait(&path)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Start file system watching for automatic updates
    pub fn start_watching(&self) -> Result<()> {
        watchers::start_watcher(
            self.project_root.clone(),
            self.registry.clone(),
            self.type_index.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_engine_fs_creation() {
        let temp_dir = TempDir::new().unwrap();
        let fs = EngineFs::new(temp_dir.path().to_path_buf());
        assert!(fs.is_ok());
    }
}
