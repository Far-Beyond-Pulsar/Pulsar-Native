//! Multiplayer UI Helpers
//!
//! Helpers for multiplayer/collaboration features.
//! Wraps engine_backend networking subsystems.

pub mod chat;
pub mod file_sync;
pub mod presence;
pub mod session;

pub use chat::*;
pub use file_sync::*;
pub use presence::*;
pub use session::*;

// Re-export backend networking types for convenience
pub use engine_backend::subsystems::networking::{
    ClientMessage, MultiuserClient, P2PConnection, ServerMessage,
};
