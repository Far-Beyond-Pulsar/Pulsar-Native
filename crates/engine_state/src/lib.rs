//! Engine State Management
//!
//! Provides thread-safe, type-safe state management for the Pulsar Engine.
//! - Typed context objects (EngineContext, WindowContext, ProjectContext)
//! - Type-safe renderer registry
//! - Communication channels
//! - Global registries

mod channels;
mod discord;

// New typed systems
pub mod context;
pub mod renderers_typed;

pub use channels::{WindowRequest, WindowRequestSender, WindowRequestReceiver, window_request_channel};
pub use discord::DiscordPresence;

// Re-export typed systems as primary API
pub use context::{EngineContext, WindowContext, ProjectContext, LaunchContext};
pub use renderers_typed::{TypedRendererHandle, TypedRendererRegistry, RendererType};

// Type alias for backward compatibility - EngineState is now EngineContext
#[deprecated(note = "Use EngineContext instead")]
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
