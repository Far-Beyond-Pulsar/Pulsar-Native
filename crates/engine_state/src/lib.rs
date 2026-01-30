//! # Engine State Management
//!
//! Provides thread-safe, type-safe state management for the Pulsar Engine.
//!
//! ## Architecture
//!
//! The engine state system is built on typed context objects instead of string-based metadata:
//!
//! - **`EngineContext`** - Main engine-wide state with typed fields
//! - **`WindowContext`** - Per-window state (window ID, type, renderer)
//! - **`ProjectContext`** - Project-specific data (path, window association)
//! - **`LaunchContext`** - Startup parameters (URI projects, verbose mode)
//!
//! ## Type Safety
//!
//! Instead of stringly-typed metadata:
//! ```ignore
//! // Old (runtime errors possible):
//! engine_state.get_metadata("current_project_path")
//! engine_state.set_metadata("latest_window_id", id.to_string())
//! ```
//!
//! Use compile-time type-safe contexts:
//! ```ignore
//! // New (compile-time safety):
//! engine_context.project.read().map(|p| p.path.clone())
//! // Window ID passed directly as parameter
//! ```
//!
//! ## Renderer Registry
//!
//! Type-safe renderer storage using enums instead of `Arc<dyn Any>`:
//! ```ignore
//! // Old (runtime panic risk):
//! renderer.downcast::<Mutex<GpuRenderer>>().unwrap()
//!
//! // New (compile-time safety):
//! handle.as_bevy::<Mutex<GpuRenderer>>() // Returns Option
//! ```

mod channels;
mod discord;
mod multiuser;

// Typed systems (primary API)
pub mod context;
pub mod renderers_typed;

pub use channels::{WindowRequest, WindowRequestSender, WindowRequestReceiver, window_request_channel};
pub use discord::DiscordPresence;

// Re-export multiuser types and functions
pub use multiuser::{
    MultiuserContext, MultiuserStatus,
    // Global access functions
    set_multiuser_context, clear_multiuser_context, get_multiuser_context,
    is_multiuser_active, are_we_host, our_peer_id, host_peer_id,
    session_id, server_url, multiuser_status, set_multiuser_status,
    add_participant, remove_participant, get_participants, participant_count,
    sync_from_engine_context,
};

// Re-export typed systems as primary API
pub use context::{EngineContext, WindowContext, ProjectContext, LaunchContext};
pub use renderers_typed::{TypedRendererHandle, TypedRendererRegistry, RendererType};

// Type alias for backward compatibility - EngineState is now EngineContext
#[deprecated(since = "0.2.0", note = "Use EngineContext instead - provides typed context fields")]
pub type EngineState = EngineContext;

// Project path storage for backward compatibility
use std::sync::OnceLock;
static PROJECT_PATH: OnceLock<String> = OnceLock::new();

/// Get the current project path (compatibility function)
///
/// Returns the project path as a static string slice.
/// This uses a separate static storage for backward compatibility.
pub fn get_project_path() -> Option<&'static str> {
    PROJECT_PATH.get().map(|s| s.as_str())
}

/// Set the current project path (compatibility function)
///
/// Stores the path both in the static storage and in EngineContext if available.
pub fn set_project_path(path: String) {
    // Update EngineContext if available
    if let Some(ctx) = EngineContext::global() {
        let project_ctx = ProjectContext::new(std::path::PathBuf::from(path.clone()));
        ctx.set_project(project_ctx);
    }

    // Also update static storage for backward compatibility
    let _ = PROJECT_PATH.set(path);
}
