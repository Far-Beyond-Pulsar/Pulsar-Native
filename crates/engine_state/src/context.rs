//! Typed Engine Context System
//!
//! Replaces the string-based metadata system with type-safe context objects.
//! Each context represents a specific domain (windows, projects, etc.) with
//! proper types instead of string key-value pairs.

use dashmap::DashMap;
use gpui::AppContext;
use pulsar_auth::AuthProfile;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use engine_fs::UserTypeRegistry;
use ui_types_common::window_types::{WindowId, WindowRequest};
use window_manager;

use crate::DiscordPresence;

use gpui::Render;
use window_manager::WindowManager;

/// Context for a specific window
#[derive(Clone)]
pub struct WindowContext {
    /// Window ID (as u64)
    pub window_id: WindowId,
    /// Type of window (Entry, Settings, ProjectEditor, etc.)
    pub window_type: WindowRequest,
}

impl WindowContext {
    pub fn new(window_id: WindowId, window_type: WindowRequest) -> Self {
        Self {
            window_id,
            window_type,
        }
    }
}

/// Context populated at startup describing whether the engine is running from a
/// source build (i.e. directly out of `target/{debug,release}/`).
///
/// When `is_source_build` is true, `source_path` holds the workspace root (the
/// directory that contains the `target/` folder).  UI subsystems can gate
/// developer-only features on this field.
#[derive(Clone, Debug, Default)]
pub struct DevContext {
    /// True when the binary lives inside a `target/{debug,release}/` tree.
    pub is_source_build: bool,
    /// Absolute path to the workspace root (parent of `target/`).
    /// `None` when `is_source_build` is false.
    pub source_path: Option<PathBuf>,
}

impl DevContext {
    /// Detect whether the current executable was launched from a Cargo output
    /// directory and, if so, return the inferred workspace root.
    pub fn detect() -> Self {
        let Ok(exe) = std::env::current_exe() else {
            return Self::default();
        };

        // Expected layout: <workspace>/target/<profile>/binary
        let profile_dir = match exe.parent() {
            Some(p) => p,
            None => return Self::default(),
        };
        let target_dir = match profile_dir.parent() {
            Some(p) => p,
            None => return Self::default(),
        };

        let profile_name = profile_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        let target_name = target_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if (profile_name == "debug" || profile_name == "release") && target_name == "target" {
            let source_path = target_dir.parent().map(|p| p.to_path_buf());
            Self {
                is_source_build: true,
                source_path,
            }
        } else {
            Self::default()
        }
    }
}

/// Context for the currently open project
#[derive(Clone, Debug)]
pub struct ProjectContext {
    /// Path to the project directory
    pub path: PathBuf,
    /// Window ID where the project is open (as u64)
    pub window_id: Option<WindowId>,
}

impl ProjectContext {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            window_id: None,
        }
    }

    pub fn with_window_id(mut self, window_id: WindowId) -> Self {
        self.window_id = Some(window_id);
        self
    }
}

/// Context for engine launch (command-line args, URI launches, etc.)
#[derive(Clone, Debug)]
pub struct LaunchContext {
    /// Project path if launched via URI scheme (pulsar://open_project/path)
    pub uri_project_path: Option<PathBuf>,
    /// Verbose logging enabled
    pub verbose: bool,
}

impl LaunchContext {
    pub fn new() -> Self {
        Self {
            uri_project_path: None,
            verbose: false,
        }
    }

    pub fn with_uri_project(mut self, path: PathBuf) -> Self {
        self.uri_project_path = Some(path);
        self
    }

    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }
}

impl Default for LaunchContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Main engine context - typed, thread-safe access to all engine state
///
/// This provides type-safe access to engine state across different domains.
/// Instead of `get_metadata("current_project_path")`, you use `context.project.read().path`.
#[derive(Clone)]
pub struct EngineContext {
    /// Per-window contexts indexed by WindowId
    pub windows: Arc<DashMap<WindowId, WindowContext>>,

    /// Multiuser session context (if in a collaborative session).
    ///
    /// Backed by [`crate::store::StateStore`] — this is the first field
    /// migrated to the generic resource system. `.read()` works exactly as
    /// before; mutation goes through [`Self::set_multiuser`] /
    /// [`Self::update_multiuser`] / [`Self::clear_multiuser`], which now
    /// notify *every* subscriber via [`crate::resource::ResourceHandle::changed`]
    /// instead of a single-consumer channel.
    pub multiuser: crate::resource::ResourceHandle<Option<crate::multiuser::MultiuserContext>>,

    /// Typed renderer registry (replaces old Arc<dyn Any> system)
    pub renderers: crate::renderers_typed::TypedRendererRegistry,

    /// Monotonically increasing window ID counter (no cross-thread ordering
    /// needed — uniqueness is all that matters for IDs).
    next_id: Arc<AtomicU64>,

    /// Generic, type-safe global resource table.
    ///
    /// This is the extension point for new engine-wide singleton state.
    /// Instead of adding a new named field here or a new `OnceLock` in some
    /// other crate, store your type here:
    pub store: crate::store::StateStore,

    /// Generic, type-safe per-window resource table.
    ///
    /// The extension point for new per-window state (replaces ad-hoc
    /// per-window registries):
    pub window_state: crate::keyed_store::KeyedStore<WindowId>,
}

impl EngineContext {
    /// Create a new engine context
    pub fn new() -> Self {
        let store = crate::store::StateStore::new();
        let multiuser = store.get_or_init::<Option<crate::multiuser::MultiuserContext>>();

        Self {
            windows: Arc::new(DashMap::new()),
            multiuser,
            renderers: crate::renderers_typed::TypedRendererRegistry::new(),
            next_id: Arc::new(AtomicU64::new(1)),
            window_state: crate::keyed_store::KeyedStore::new(),
            store,
        }
    }

    /// Allocate the next unique window ID
    pub fn next_window_id(&self) -> WindowId {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Register a window context
    pub fn register_window(&self, window_id: WindowId, context: WindowContext) {
        self.windows.insert(window_id, context);
    }

    /// Unregister a window
    pub fn unregister_window(&self, window_id: &WindowId) -> Option<WindowContext> {
        self.windows.remove(window_id).map(|(_, ctx)| ctx)
    }

    /// Convenience wrapper that either routes through the WindowManager
    /// Create a window through the window manager using a generic content builder.
    /// This is the preferred method as it preserves the view type and allows Root
    /// to be the actual window root.
    pub fn create_window<V, F>(
        &self,
        window_type: WindowRequest,
        options: gpui::WindowOptions,
        content_builder: F,
        cx: &mut gpui::App,
    ) -> Result<(WindowId, gpui::AnyWindowHandle), window_manager::WindowError>
    where
        V: gpui::Render + 'static,
        F: FnOnce(&mut gpui::Window, &mut gpui::App) -> gpui::Entity<V> + Send + 'static,
    {
        use gpui::UpdateGlobal;
        WindowManager::update_global(cx, |wm, cx| {
            wm.create_window(window_type, options, content_builder, cx)
        })
    }

    /// Get window count
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Set current project
    pub fn set_project(&self, project: ProjectContext) {
        self.store
            .get_or_init::<Option<ProjectContext>>()
            .set(Some(project));
    }

    /// Clear current project
    pub fn clear_project(&self) {
        self.store
            .get_or_init::<Option<ProjectContext>>()
            .set(None);
    }

    /// Initialize Discord Rich Presence
    pub fn init_discord(&self, application_id: impl Into<String>) -> anyhow::Result<()> {
        let presence = DiscordPresence::new(application_id);
        presence.connect()?;
        self.store
            .get_or_init::<Option<DiscordPresence>>()
            .set(Some(presence));
        Ok(())
    }

    /// Get Discord presence handle
    pub fn discord(&self) -> Option<DiscordPresence> {
        self.store
            .get_or_init::<Option<DiscordPresence>>()
            .read()
            .clone()
    }

    /// Update Discord presence
    pub fn update_discord_presence(
        &self,
        project_name: Option<String>,
        tab_name: Option<String>,
        file_path: Option<String>,
    ) {
        let handle = self.store.get_or_init::<Option<DiscordPresence>>();
        let guard = handle.read();
        if let Some(discord) = guard.as_ref() {
            discord.update_all(project_name, tab_name, file_path);
        }
    }

    /// Set global user type registry
    pub fn set_user_types(&self, user_types: Arc<UserTypeRegistry>) {
        self.store
            .get_or_init::<Option<Arc<UserTypeRegistry>>>()
            .set(Some(user_types));
    }

    /// Set authenticated user profile.
    pub fn set_auth_profile(&self, profile: AuthProfile) {
        self.store
            .get_or_init::<Option<AuthProfile>>()
            .set(Some(profile));
    }

    /// Clear authenticated user profile.
    pub fn clear_auth_profile(&self) {
        self.store
            .get_or_init::<Option<AuthProfile>>()
            .set(None);
    }

    /// Get authenticated user profile.
    pub fn auth_profile(&self) -> Option<AuthProfile> {
        self.store
            .get_or_init::<Option<AuthProfile>>()
            .read()
            .clone()
    }

    /// Get global user type registry
    pub fn user_types(&self) -> Option<Arc<UserTypeRegistry>> {
        self.store
            .get_or_init::<Option<Arc<UserTypeRegistry>>>()
            .read()
            .clone()
    }

    /// Set multiuser session context
    ///
    /// Call this when joining or creating a multiuser session.
    /// This makes the session details available to all subsystems.
    ///
    /// # Example
    /// ```ignore
    /// let multiuser_ctx = MultiuserContext::new(
    ///     "ws://localhost:8080",
    ///     "session-123",
    ///     "peer-abc",
    ///     "peer-abc"
    /// ).with_status(MultiuserStatus::Connected);
    ///
    /// engine_context.set_multiuser(multiuser_ctx);
    /// ```
    pub fn set_multiuser(&self, context: crate::multiuser::MultiuserContext) {
        self.multiuser.set(Some(context));
    }

    /// Mutate multiuser context in place if active.
    ///
    /// Returns `true` when a context existed and was updated. Subscribers
    /// (via [`crate::resource::ResourceHandle::changed`]) are only notified
    /// when an update actually happened.
    pub fn update_multiuser<F>(&self, update: F) -> bool
    where
        F: FnOnce(&mut crate::multiuser::MultiuserContext),
    {
        if self.multiuser.read().is_none() {
            return false;
        }
        self.multiuser.update(|guard| {
            if let Some(ctx) = guard.as_mut() {
                update(ctx);
            }
        });
        true
    }

    /// Clear multiuser session context
    ///
    /// Call this when disconnecting from a session.
    pub fn clear_multiuser(&self) {
        self.multiuser.set(None);
    }

    /// Get multiuser session context (if active)
    pub fn multiuser(&self) -> Option<crate::multiuser::MultiuserContext> {
        self.multiuser.read().clone()
    }

    /// Check if currently in a multiuser session
    pub fn is_multiuser_active(&self) -> bool {
        self.multiuser.read().is_some()
    }

    /// Check if we're the host of the current session
    pub fn are_we_pulsar_studio(&self) -> bool {
        self.multiuser
            .read()
            .as_ref()
            .map(|ctx| ctx.is_host)
            .unwrap_or(false)
    }

    /// Update multiuser connection status
    pub fn set_multiuser_status(&self, status: crate::multiuser::MultiuserStatus) {
        let _ = self.update_multiuser(|ctx| ctx.set_status(status));
    }

    /// Add a participant to the current multiuser session
    pub fn add_multiuser_participant(&self, peer_id: impl Into<String>) {
        let peer_id = peer_id.into();
        let _ = self.update_multiuser(|ctx| ctx.add_participant(peer_id));
    }

    /// Replace participant list for the active session.
    pub fn set_multiuser_participants(&self, participants: Vec<String>) {
        let _ = self.update_multiuser(|ctx| {
            ctx.participants = participants;
        });
    }

    pub fn set_multiuser_participant_profiles(
        &self,
        participants: Vec<crate::multiuser::MultiuserParticipant>,
    ) {
        let _ = self.update_multiuser(|ctx| {
            ctx.participant_profiles = participants;
        });
    }

    pub fn set_multiuser_latency_ms(&self, latency_ms: Option<u32>) {
        let _ = self.update_multiuser(|ctx| {
            ctx.latency_ms = latency_ms;
        });
    }

    /// Remove a participant from the current multiuser session
    pub fn remove_multiuser_participant(&self, peer_id: &str) {
        let _ = self.update_multiuser(|ctx| ctx.remove_participant(peer_id));
    }

    /// Notify listeners that the multiuser snapshot changed.
    pub fn notify_multiuser_changed(&self) {
        self.multiuser.update(|_| {});
    }

    /// Set as global instance (for GPUI views that need global access)
    pub fn set_global(self) {
        GLOBAL_CONTEXT.set(self);
    }

    /// Get global instance
    pub fn global() -> Option<&'static Self> {
        GLOBAL_CONTEXT.get()
    }
}

impl Default for EngineContext {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_CONTEXT: OnceLock<EngineContext> = OnceLock::new();

// (legacy metadata system removed)
pub mod migration {

    /// Extract window ID from metadata string (used during migration)
    pub fn parse_window_id_u64(id_str: &str) -> Option<u64> {
        id_str.parse::<u64>().ok()
    }

    /// Format window ID as string (used during migration)
    pub fn format_window_id_u64(id: u64) -> String {
        id.to_string()
    }

    /// Map old metadata key to new context access
    ///
    /// This documents the migration path from string metadata to typed contexts.
    ///
    /// Old: `engine_state.get_metadata("current_project_path")`
    /// New: `engine_context.project.read().as_ref().map(|p| &p.path)`
    ///
    /// Old: `engine_state.set_metadata("uri_project_path", path)`
    /// New: `engine_context.launch.write().uri_project_path = Some(path)`
    ///
    /// Old: `engine_state.get_metadata("latest_window_id")`
    /// New: Use the actual WindowId from the window creation event
    pub struct MetadataKeyMapping;

    impl MetadataKeyMapping {
        pub const URI_PROJECT_PATH: &'static str = "uri_project_path";
        pub const CURRENT_PROJECT_PATH: &'static str = "current_project_path";
        pub const CURRENT_PROJECT_WINDOW_ID: &'static str = "current_project_window_id";
        pub const LATEST_WINDOW_ID: &'static str = "latest_window_id";
        pub const HAS_PENDING_VIEWPORT_RENDERER: &'static str = "has_pending_viewport_renderer";
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_window_context_creation() {
        // Note: WindowId is now u64, so we can test directly
        let _window_id: WindowId = 42;
        // Would need WindowRequest to create full WindowContext
        // Placeholder for when we have integration tests
    }

    #[test]
    fn test_project_context() {
        let project = ProjectContext::new(PathBuf::from("/test/project"));
        assert_eq!(project.path, PathBuf::from("/test/project"));
        assert_eq!(project.window_id, None);
    }

    #[test]
    fn test_launch_context() {
        let launch = LaunchContext::new()
            .with_uri_project(PathBuf::from("/uri/project"))
            .with_verbose(true);

        assert_eq!(launch.uri_project_path, Some(PathBuf::from("/uri/project")));
        assert!(launch.verbose);
    }

    #[test]
    fn test_engine_context_window_count() {
        let context = EngineContext::new();
        assert_eq!(context.window_count(), 0);

        // Would need real WindowId to test further
    }

    #[test]
    fn test_engine_context_project() {
        let context = EngineContext::new();
        let project_handle = context.store.get_or_init::<Option<ProjectContext>>();

        assert!(project_handle.read().is_none());

        let project = ProjectContext::new(PathBuf::from("/test"));
        context.set_project(project.clone());

        assert!(project_handle.read().is_some());
        assert_eq!(
            project_handle.read().as_ref().unwrap().path,
            PathBuf::from("/test")
        );

        context.clear_project();
        assert!(project_handle.read().is_none());
    }
}
