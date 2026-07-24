//! Networking Subsystem
//!
//! Handles all network communication:
//! - WebSocket multiuser client
//! - P2P peer connections
//! - Git sync protocol
//! - Simple file sync

#[cfg(feature = "vcs")]
pub mod git_sync;
#[cfg(feature = "networking")]
pub mod multiuser;
pub mod p2p;
#[cfg(feature = "vcs")]
pub mod simple_sync;

#[cfg(feature = "vcs")]
pub use git_sync::*;
#[cfg(feature = "networking")]
pub use multiuser::{ClientMessage, MultiuserClient, ServerMessage};
pub use p2p::P2PConnection;
#[cfg(feature = "vcs")]
pub use simple_sync::{FileEntry, FileManifest, SyncDiff};
