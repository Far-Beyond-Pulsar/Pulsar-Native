//! Filesystem provider abstraction and implementations
//!
//! This module contains the core [`FsProvider`] trait and concrete implementations
//! for both local disk and remote (HTTP-based) filesystems.

mod local;
#[cfg(feature = "p2p")]
pub mod p2p;
mod provider_trait;
#[cfg(feature = "remote")]
mod remote;

// Re-export all public types
pub use local::LocalFsProvider;
#[cfg(feature = "p2p")]
pub use p2p::P2pFsProvider;
pub use provider_trait::{FsEntry, FsMetadata, FsProvider, ManifestEntry};
#[cfg(feature = "remote")]
pub use remote::{RemoteConfig, RemoteFsProvider};
