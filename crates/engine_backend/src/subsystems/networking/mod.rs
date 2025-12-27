//! Networking Subsystem
//!
//! Handles all network communication:
//! - WebSocket multiuser client
//! - P2P peer connections
//! - Git sync protocol
//! - Simple file sync

pub mod git_sync;
pub mod multiuser;
pub mod p2p;
pub mod simple_sync;

pub use git_sync::*;
pub use multiuser::{ClientMessage, MultiuserClient, ServerMessage};
pub use p2p::P2PConnection;
pub use simple_sync::{FileManifest, FileEntry, SyncDiff};
