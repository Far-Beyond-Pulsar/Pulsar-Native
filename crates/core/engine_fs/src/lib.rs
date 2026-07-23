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
#[cfg(feature = "editor")]
pub mod asset_index;
#[cfg(feature = "editor")]
mod engine_fs;
pub mod events;
#[cfg(feature = "editor")]
pub mod operations;
pub mod providers;
#[cfg(feature = "editor")]
mod scanner;
#[cfg(feature = "editor")]
pub mod templates;
#[cfg(feature = "editor")]
pub mod thumbnails;
#[cfg(feature = "editor")]
pub mod tooling;
#[cfg(feature = "editor")]
pub mod user_types;
pub mod virtual_fs;
#[cfg(feature = "editor")]
pub mod watchers;

// Re-export main types
#[cfg(feature = "editor")]
pub use asset_index::{AssetIndex, AssetInfo};
#[cfg(feature = "editor")]
pub use engine_fs::EngineFs;
#[cfg(feature = "editor")]
pub use user_types::{UserTypeInfo, UserTypeRegistry};

// Re-export provider types
#[cfg(feature = "p2p")]
pub use providers::P2pFsProvider;
pub use providers::{FsEntry, FsMetadata, FsProvider, LocalFsProvider, ManifestEntry};
#[cfg(feature = "remote")]
pub use providers::{RemoteConfig, RemoteFsProvider};

// Re-export operations
pub use events::{emit, subscribe, FsChangeKind, FsEvent};
#[cfg(feature = "editor")]
pub use operations::AssetOperations;

// Re-export template types
#[cfg(feature = "editor")]
pub use templates::{AssetCategory, AssetKind};

// Re-export commonly used virtual_fs functions
pub use virtual_fs::{cloud_join, is_cloud_path};

#[cfg(all(test, feature = "editor"))]
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
