//! Multiplayer UI
//!
//! Real-time collaboration and multiplayer features

mod chat;
mod connection;
mod diff;
mod file_sync;
mod file_sync_ui;
mod presence;
mod session;
mod state;
mod traits;
mod types;
mod ui;
mod utils;

// Re-export main types
pub use diff::*;
pub use state::MultiplayerWindow;
pub use types::*;
