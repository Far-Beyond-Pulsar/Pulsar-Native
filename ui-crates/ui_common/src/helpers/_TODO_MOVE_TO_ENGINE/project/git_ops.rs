//! Git Operations Helpers
//!
//! Helpers for git operations (clone, commit, push, pull).
//! Wraps engine_backend networking git_sync.

use engine_backend::subsystems::networking as backend_networking;
pub use backend_networking::git_sync::*;

// Additional UI-specific git helpers will go here
