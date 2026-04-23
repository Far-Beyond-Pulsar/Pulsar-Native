//! Asset operations
//!
//! Handles all file operations (create, update, delete) and maintains index consistency.
//! Split into type-specific and general operations for better organization.

mod general_ops;
mod type_ops;

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use type_db::TypeDatabase;

use crate::templates::AssetKind;

// Re-export operation handlers
pub use general_ops::GeneralOperations;
pub use type_ops::TypeOperations;

/// Main asset operations coordinator
pub struct AssetOperations {
    type_ops: TypeOperations,
    general_ops: GeneralOperations,
}

impl AssetOperations {
    pub fn new(project_root: PathBuf, type_database: Arc<TypeDatabase>) -> Self {
        Self {
            type_ops: TypeOperations::new(project_root.clone(), type_database.clone()),
            general_ops: GeneralOperations::new(project_root, type_database),
        }
    }

    // ── Type Alias Operations ─────────────────────────────────────────────────

    /// Create a new type alias file
    pub fn create_type_alias(&self, name: &str, content: &str) -> Result<PathBuf> {
        self.type_ops.create_type_alias(name, content)
    }

    /// Update an existing type alias file
    pub fn update_type_alias(&self, file_path: &PathBuf, content: &str) -> Result<()> {
        self.type_ops.update_type_alias(file_path, content)
    }

    /// Delete a type alias file
    pub fn delete_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        self.type_ops.delete_type_alias(file_path)
    }

    /// Register an existing type alias file (for scanning)
    pub fn register_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        self.type_ops.register_type_alias(file_path)
    }

    /// Move/rename a type alias file
    pub fn move_type_alias(&self, old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
        self.type_ops.move_type_alias(old_path, new_path)
    }

    // ── General Asset Operations ──────────────────────────────────────────────

    /// Create a new asset of any kind
    pub fn create_asset(
        &self,
        kind: AssetKind,
        name: &str,
        custom_dir: Option<&str>,
    ) -> Result<PathBuf> {
        self.general_ops.create_asset(kind, name, custom_dir)
    }

    /// Delete any asset file
    pub fn delete_asset(&self, file_path: &PathBuf) -> Result<()> {
        self.general_ops.delete_asset(file_path)
    }

    /// Rename/move any asset file
    pub fn move_asset(&self, old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
        self.general_ops.move_asset(old_path, new_path)
    }
}
