//! Main engine filesystem manager
//!
//! Coordinates all asset operations and maintains type database.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use type_db::TypeDatabase;

use crate::operations::AssetOperations;
use crate::scanner::ProjectScanner;
use crate::watchers;

/// The main engine filesystem manager
pub struct EngineFs {
    project_root: PathBuf,
    type_database: Arc<TypeDatabase>,
    operations: AssetOperations,
    scanner: ProjectScanner,
}

impl EngineFs {
    /// Create a new EngineFs instance for a project
    pub fn new(project_root: PathBuf) -> Result<Self> {
        let type_database = Arc::new(TypeDatabase::new());
        let operations = AssetOperations::new(
            project_root.clone(),
            type_database.clone(),
        );
        let scanner = ProjectScanner::new(
            project_root.clone(),
            type_database.clone(),
        );

        let mut fs = Self {
            project_root,
            type_database,
            operations,
            scanner,
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
        self.scanner.scan_project()
    }

    /// Start file system watching for automatic updates
    /// Note: Currently only watches for file removals. Rescan project to detect new/modified files.
    pub fn start_watching(&self) -> Result<()> {
        watchers::start_watcher(
            self.project_root.clone(),
            self.type_database.clone(),
        )?;

        tracing::trace!("Started filesystem watcher for project at {:?}", self.project_root);

        Ok(())
    }
}
