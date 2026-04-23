//! Multiplayer UI
//!
//! Real-time collaboration and multiplayer features

mod chat;
mod connection;
mod diff;
mod diff_viewer;
mod file_sync;
mod file_sync_ui;
mod peer_state;
mod presence;
mod session;
mod state;
mod sync_protocol;
mod traits;
mod types;
mod ui;
mod utils;

// Re-export main types
pub use diff::*;
pub use diff_viewer::{DiffFileEntry, DiffViewer};
pub use state::MultiplayerWindow;
pub use types::*;
