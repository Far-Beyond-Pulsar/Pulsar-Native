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
//! handle.as_helio::<Mutex<GpuRenderer>>() // Returns Option
//! ```

mod discord;
mod multiuser;

// Typed systems (primary API)
pub mod context;
pub mod renderers_typed;

// Settings system — backed by PulsarConfig
pub mod settings;
pub mod settings_defaults;

pub use discord::DiscordPresence;

// Re-export multiuser types
pub use multiuser::{MultiuserContext, MultiuserStatus};

// Re-export typed systems as primary API
pub use context::{EngineContext, LaunchContext, ProjectContext, WindowContext};
pub use renderers_typed::{RendererType, TypedRendererHandle, TypedRendererRegistry};

// Re-export settings system (PulsarConfig surface)
pub use settings::{
    global_config,
    // PulsarConfig types
    ChangeEvent,
    Color,
    ConfigError,
    ConfigManager,
    ConfigStore,
    ConfigValue,
    DropdownOption,
    FieldType,
    GlobalSettings,
    ListenerId,
    NamespaceSchema,
    OwnerHandle,
    PersistError,
    ProjectSettings,
    SchemaEntry,
    SearchResult,
    SettingInfo,
    Validator,
    NS_EDITOR,
    NS_PROJECT,
};
pub use settings_defaults::register_default_settings;

// Type alias for backward compatibility - EngineState is now EngineContext
#[deprecated(
    since = "0.2.0",
    note = "Use EngineContext instead - provides typed context fields"
)]
pub type EngineState = EngineContext;

/// Set the current project path.
///
/// Updates `EngineContext::project` so that all subsystems that read the
/// context get the new value immediately.  The old write-once `OnceLock`
/// static has been removed — use `EngineContext::global()` to read the path.
pub fn set_project_path(path: String) {
    if let Some(ctx) = EngineContext::global() {
        let project_ctx = ProjectContext::new(std::path::PathBuf::from(path));
        ctx.set_project(project_ctx);
    }
}

/// Get the current project path from EngineContext.
pub fn get_project_path() -> Option<String> {
    EngineContext::global().and_then(|ctx| {
        ctx.project
            .read()
            .as_ref()
            .map(|p| p.path.to_string_lossy().into_owned())
    })
}

pub use ui_types_common::window_types::{WindowId, WindowRequest};
