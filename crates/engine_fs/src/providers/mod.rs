//! Filesystem provider abstraction and implementations
//!
//! This module contains the core [`FsProvider`] trait and concrete implementations
//! for both local disk and remote (HTTP-based) filesystems.

mod provider_trait;
mod local;
mod remote;

// Re-export all public types
pub use provider_trait::{FsProvider, FsEntry, FsMetadata, ManifestEntry};
pub use local::LocalFsProvider;
pub use remote::{RemoteFsProvider, RemoteConfig};
