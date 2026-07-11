//! Multiplayer UI
//!
//! Real-time collaboration and multiplayer features

mod chat;
mod components;
mod connection;
mod diff;
mod diff_viewer;
mod file_sync;
mod file_sync_ui;
mod handlers;
mod peer_state;
mod presence;
pub mod screen;
mod session;
mod sync_protocol;
mod utils;

// Re-export main types
pub use screen::MultiplayerWindow;
pub use diff::*;
pub use diff_viewer::{DiffFileEntry, DiffViewer};
pub use utils::types::*;
