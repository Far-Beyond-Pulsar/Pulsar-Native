//! Main engine filesystem manager
//!
//! Coordinates all asset operations and maintains type database.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

use crate::asset_index::AssetIndex;
use crate::operations::AssetOperations;
use crate::scanner::ProjectScanner;
use crate::user_types::UserTypeRegistry;
use crate::watchers;

/// The main engine filesystem manager
pub struct EngineFs {
    project_root: PathBuf,
    asset_index: Arc<AssetIndex>,
    user_types: Arc<UserTypeRegistry>,
    operations: AssetOperations,
    scanner: ProjectScanner,
}

impl EngineFs {
    /// Create a new EngineFs instance for a project
    pub fn new(project_root: PathBuf) -> Result<Self> {
        let asset_index = Arc::new(AssetIndex::new());
        let user_types = Arc::new(UserTypeRegistry::new());
        let operations = AssetOperations::new(
            project_root.clone(),
            asset_index.clone(),
            user_types.clone(),
        );
        let scanner = ProjectScanner::new(
            project_root.clone(),
            asset_index.clone(),
            user_types.clone(),
        );

        let mut fs = Self {
            project_root,
            asset_index,
            user_types,
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

    /// Get the general project asset index
    pub fn asset_index(&self) -> &Arc<AssetIndex> {
        &self.asset_index
    }

    /// Get the user-defined type registry
    pub fn user_types(&self) -> &Arc<UserTypeRegistry> {
        &self.user_types
    }

    /// Get asset operations handler
    pub fn operations(&self) -> &AssetOperations {
        &self.operations
    }

    /// Scan the entire project and build the asset index and user type registry
    pub fn scan_project(&mut self) -> Result<()> {
        self.scanner.scan_project()
    }

    /// Start file system watching for automatic updates
    /// Note: Currently only watches for file removals. Rescan project to detect new/modified files.
    pub fn start_watching(&self) -> Result<()> {
        watchers::start_watcher(
            self.project_root.clone(),
            self.asset_index.clone(),
            self.user_types.clone(),
        )?;

        tracing::trace!(
            "Started filesystem watcher for project at {:?}",
            self.project_root
        );

        Ok(())
    }
}
