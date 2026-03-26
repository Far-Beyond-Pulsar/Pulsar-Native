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
pub mod providers;
pub mod virtual_fs;
pub mod operations;
pub mod templates;
pub mod watchers;
mod engine_fs;
mod scanner;

// Re-export main types
pub use engine_fs::EngineFs;

// Re-export provider types
pub use providers::{
    FsProvider, FsEntry, FsMetadata, ManifestEntry,
    LocalFsProvider, RemoteFsProvider, RemoteConfig,
};

// Re-export operations
pub use operations::AssetOperations;

// Re-export template types
pub use templates::{AssetKind, AssetCategory};

// Re-export commonly used virtual_fs functions
pub use virtual_fs::{is_cloud_path, cloud_join};

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
