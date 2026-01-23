//! Engine Filesystem Layer
//!
//! Centralized asset management and indexing system for Pulsar Engine.
//! Handles all file operations and maintains up-to-date indexes for quick lookups.

pub mod watchers;
pub mod operations;
pub mod asset_templates;

pub use operations::AssetOperations;
pub use asset_templates::{AssetKind, AssetCategory};

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use type_db::TypeDatabase;

/// The main engine filesystem manager
/// Coordinates all asset operations and maintains type database
pub struct EngineFs {
    project_root: PathBuf,
    type_database: Arc<TypeDatabase>,
    operations: AssetOperations,
}

impl EngineFs {
    /// Create a new EngineFs instance for a project
    pub fn new(project_root: PathBuf) -> Result<Self> {
        let type_database = Arc::new(TypeDatabase::new());
        let operations = AssetOperations::new(
            project_root.clone(),
            type_database.clone(),
        );

        let mut fs = Self {
            project_root,
            type_database,
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

    /// Get the type database
    pub fn type_database(&self) -> &Arc<TypeDatabase> {
        &self.type_database
    }

    /// Get asset operations handler
    pub fn operations(&self) -> &AssetOperations {
        &self.operations
    }

    /// Scan the entire project and build type database
    pub fn scan_project(&mut self) -> Result<()> {
        use walkdir::WalkDir;

        // Clear existing type database
        self.type_database.clear();

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
        use type_db::TypeKind;

        // Check if it's a JSON file
        if let Some(extension) = path.extension() {
            if extension == "json" {
                // Get the filename (e.g., "struct.json", "enum.json", "trait.json", "alias.json")
                if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
                    // Get the parent folder name as the type name (e.g., "GameState", "Drawable")
                    let type_name = path.parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let type_kind = match file_name {
                        "struct.json" => Some(TypeKind::Struct),
                        "enum.json" => Some(TypeKind::Enum),
                        "trait.json" => Some(TypeKind::Trait),
                        _ if file_name.contains("alias") => {
                            // Handle alias.json or *.alias.json
                            self.operations.register_type_alias(&path)?;
                            return Ok(());
                        }
                        _ => None,
                    };

                    if let Some(kind) = type_kind {
                        if let Err(e) = self.type_database.register_with_path(
                            type_name.clone(),
                            path.clone(),
                            kind,
                            None,
                            Some(format!("{:?}: {}", kind, type_name)),
                            None,
                        ) {
                            tracing::warn!("Failed to register type '{}': {:?}", type_name, e);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Start file system watching for automatic updates
    pub fn start_watching(&self) -> Result<()> {
        let fs_watcher = watchers::start_watcher(
            self.project_root.clone(),
            self.type_database.clone(),
        )?;

        println!("Started filesystem watcher for project at {:?}", self.project_root);

        Ok(fs_watcher)
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
