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

        if let Some(extension) = path.extension() {
            let ext_str = extension.to_string_lossy();
            match ext_str.as_ref() {
                "alias" | "json" if path.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.contains("alias"))
                    .unwrap_or(false) =>
                {
                    // Type alias file
                    self.operations.register_type_alias(&path)?;
                }
                "struct" | "enum" | "trait" => {
                    // Register struct, enum, or trait in type database
                    let name = path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let type_kind = match ext_str.as_ref() {
                        "struct" => TypeKind::Struct,
                        "enum" => TypeKind::Enum,
                        "trait" => TypeKind::Trait,
                        _ => return Ok(()),
                    };

                    self.type_database.register_with_path(
                        name.clone(),
                        path.clone(),
                        type_kind,
                        None,
                        Some(format!("{:?}: {}", type_kind, name)),
                        None,
                    );
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
            self.type_database.clone(),
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
