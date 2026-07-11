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
//! engine_context.store.get_or_init::<Option<ProjectContext>>().read().as_ref().map(|p| p.path.clone())
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
//!
//! ## Generic Resource System (extension point for new state)
//!
//! [`EngineContext`] also exposes a generic, type-safe resource system —
//! [`store`] / [`StateStore`] for global singletons and [`keyed_store`] /
//! [`KeyedStore`] for per-window (or otherwise keyed) state. This is the
//! preferred home for **any new piece of engine/editor state**: instead of
//! adding another named field to `EngineContext` or another hand-rolled
//! `static FOO: OnceLock<RwLock<T>>` in some other crate, store your type
//! here. No upfront registration is required — the first call to
//! `get_or_init` creates it via [`Default`].
//!
//! ```ignore
//! #[derive(Default)]
//! struct GizmoSettings { snap_translation: f32 }
//!
//! let ctx = EngineContext::global().unwrap();
//!
//! // Global singleton, created on first use:
//! let gizmo = ctx.store.get_or_init::<GizmoSettings>();
//! gizmo.update(|g| g.snap_translation = 0.5);
//!
//! // Per-window state, keyed by WindowId:
//! #[derive(Default)]
//! struct PanelLayout { sidebar_width: f32 }
//! let layout = ctx.window_state.get_or_init::<PanelLayout>(&window_id);
//!
//! // React to changes from any number of independent listeners:
//! gizmo.changed().await;
//! ```
//!
//! See `EngineContext::multiuser` for a real migration of an existing field
//! onto this system (it replaced a single-consumer `smol::channel` bus with
//! multi-listener [`ResourceHandle::changed`]).

mod discord;
mod multiuser;

// Typed systems (primary API)
pub mod context;
pub mod renderers_typed;

// Generic, type-safe arbitrary state system
pub mod keyed_store;
pub mod resource;
pub mod store;

// Settings system — backed by PulsarConfig
pub mod settings;
pub mod settings_defaults;

pub use discord::DiscordPresence;
pub use pulsar_auth::AuthProfile;

// Re-export multiuser types
pub use multiuser::{
    MultiuserContext, MultiuserMode, MultiuserParticipant, MultiuserStatus, RelayConnectionMode,
};

// Re-export typed systems as primary API
pub use context::{DevContext, EngineContext, LaunchContext, ProjectContext, WindowContext};
pub use keyed_store::KeyedStore;
pub use renderers_typed::{RendererType, TypedRendererHandle, TypedRendererRegistry};
pub use resource::{Resource, ResourceHandle, WriteGuard};
pub use store::StateStore;

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
        ctx.store
            .get_or_init::<Option<ProjectContext>>()
            .read()
            .as_ref()
            .map(|p| p.path.to_string_lossy().into_owned())
    })
}

pub use ui_types_common::window_types::{WindowId, WindowRequest};
