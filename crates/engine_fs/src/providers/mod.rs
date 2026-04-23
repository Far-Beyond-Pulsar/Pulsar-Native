//! Filesystem provider abstraction and implementations
//!
//! This module contains the core [`FsProvider`] trait and concrete implementations
//! for both local disk and remote (HTTP-based) filesystems.

mod local;
mod provider_trait;
mod remote;

// Re-export all public types
pub use local::LocalFsProvider;
pub use provider_trait::{FsEntry, FsMetadata, FsProvider, ManifestEntry};
pub use remote::{RemoteConfig, RemoteFsProvider};
