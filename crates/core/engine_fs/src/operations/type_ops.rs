//! Type-specific asset operations (type aliases)
//!
//! Thin wrapper around [`crate::user_types::UserTypeRegistry`] that scopes
//! operations to a project root.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;

use crate::user_types::UserTypeRegistry;

/// Type alias operations handler
pub struct TypeOperations {
    project_root: PathBuf,
    user_types: Arc<UserTypeRegistry>,
}

impl TypeOperations {
    pub fn new(project_root: PathBuf, user_types: Arc<UserTypeRegistry>) -> Self {
        Self {
            project_root,
            user_types,
        }
    }

    /// Create a new type alias file
    pub fn create_type_alias(&self, name: &str, content: &str) -> Result<PathBuf> {
        self.user_types
            .create_type_alias(&self.project_root, name, content)
    }

    /// Update an existing type alias file
    pub fn update_type_alias(&self, file_path: &PathBuf, content: &str) -> Result<()> {
        self.user_types.update_type_alias(file_path, content)
    }

    /// Delete a type alias file
    pub fn delete_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        self.user_types.delete_type_alias(file_path)
    }

    /// Register an existing type alias file (for scanning)
    pub fn register_type_alias(&self, file_path: &PathBuf) -> Result<()> {
        self.user_types.register_alias_file(file_path)?;
        Ok(())
    }

    /// Move/rename a type alias file
    pub fn move_type_alias(&self, old_path: &PathBuf, new_path: &PathBuf) -> Result<()> {
        self.user_types.move_type_alias(old_path, new_path)
    }
}
