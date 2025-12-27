//! UI Helper Layer
//!
//! This module provides a clean abstraction between UI components and backend systems.
//! Helpers encapsulate common operations and reduce boilerplate in window implementations.
//!
//! ## Organization
//!
//! - `window/` - Window-specific helpers (entry, editor, settings, etc.)
//! - `editor/` - Editor operation helpers (level, script, blueprint, DAW)
//! - `project/` - Project management helpers (creation, loading, git)
//! - `multiplayer/` - Multiplayer UI helpers (session, chat, sync)

pub mod editor;
// TODO: Move these to engine crate - they depend on backend
// pub mod multiplayer;
// pub mod project;
pub mod window;

// Re-export common types
pub use window::*;
