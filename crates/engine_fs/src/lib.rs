//! Engine Filesystem Layer
//!
//! Centralized asset management and indexing system for Pulsar Engine.
//! Handles all file operations and maintains up-to-date indexes for quick lookups.
//!
//! ## Architecture
//!
//! The crate is organized into several modules:
//! - [`providers`] - Filesystem provider abstraction (local and remote)
//! - [`virtual_fs`] - Global virtual filesystem with path utilities
//! - [`operations`] - Asset CRUD operations (create, update, delete, move)
//! - [`templates`] - Asset templates and template generation
//! - [`watchers`] - File system watching for automatic updates
//! - [`engine_fs`] - Main coordinator struct
//! - [`scanner`] - Project scanning and indexing
//!
//! ## Remote file editing
//!
//! When the editor opens a cloud project the active provider is swapped to a
//! [`RemoteFsProvider`] via [`virtual_fs::set_provider`].  All code that
//! previously called `std::fs` can instead call the free functions in
//! [`virtual_fs`] (or use [`virtual_fs::provider()`] for bulk access) and
//! will automatically target the right backend.

// Module declarations
mod engine_fs;
pub mod operations;
pub mod providers;
mod scanner;
pub mod templates;
pub mod virtual_fs;
pub mod watchers;

// Re-export main types
pub use engine_fs::EngineFs;

// Re-export provider types
pub use providers::{
    FsEntry, FsMetadata, FsProvider, LocalFsProvider, ManifestEntry, RemoteConfig, RemoteFsProvider,
};

// Re-export operations
pub use operations::AssetOperations;

// Re-export template types
pub use templates::{AssetCategory, AssetKind};

// Re-export commonly used virtual_fs functions
pub use virtual_fs::{cloud_join, is_cloud_path};

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
