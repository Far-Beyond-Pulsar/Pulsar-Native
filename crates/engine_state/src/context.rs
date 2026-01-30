//! Typed Engine Context System
//!
//! Replaces the string-based metadata system with type-safe context objects.
//! Each context represents a specific domain (windows, projects, etc.) with
//! proper types instead of string key-value pairs.

use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use dashmap::DashMap;
use type_db::TypeDatabase;

use crate::{WindowRequest, DiscordPresence, WindowRequestSender};

/// Window ID type (u64 for cross-platform compatibility)
///
/// This is used instead of winit::window::WindowId to keep engine_state
/// decoupled from the windowing system. The engine crate handles conversion
/// between winit::window::WindowId and u64.
pub type WindowId = u64;

/// Context for a specific window
#[derive(Clone)]
pub struct WindowContext {
    /// Window ID (as u64)
    pub window_id: WindowId,
    /// Type of window (Entry, Settings, ProjectEditor, etc.)
    pub window_type: WindowRequest,
    /// Associated renderer handle (typed, not Arc<dyn Any>)
    pub renderer: Option<Arc<dyn std::any::Any + Send + Sync>>, // Will be replaced with TypedRendererHandle in Phase 1
}

impl WindowContext {
    pub fn new(window_id: WindowId, window_type: WindowRequest) -> Self {
        Self {
            window_id,
            window_type,
            renderer: None,
        }
    }

    pub fn with_renderer(mut self, renderer: Arc<dyn std::any::Any + Send + Sync>) -> Self {
        self.renderer = Some(renderer);
        self
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

/// Main engine context - replaces EngineState's string metadata system
///
/// This provides type-safe access to engine state across different domains.
/// Instead of `get_metadata("current_project_path")`, you use `context.project.read().path`.
#[derive(Clone)]
pub struct EngineContext {
    /// Per-window contexts indexed by WindowId
    pub windows: Arc<DashMap<WindowId, WindowContext>>,

    /// Current project context (if any)
    pub project: Arc<RwLock<Option<ProjectContext>>>,

    /// Launch parameters (URI, command-line args, etc.)
    pub launch: Arc<RwLock<LaunchContext>>,

    /// Discord Rich Presence integration
    pub discord: Arc<RwLock<Option<DiscordPresence>>>,

    /// Multiuser session context (if in a collaborative session)
    pub multiuser: Arc<RwLock<Option<crate::multiuser::MultiuserContext>>>,

    /// Global type database for reflection
    pub type_database: Arc<RwLock<Option<Arc<TypeDatabase>>>>,

    /// Window request channel sender
    pub window_sender: Arc<RwLock<Option<WindowRequestSender>>>,

    /// Window count
    pub window_count: Arc<parking_lot::Mutex<usize>>,

    /// Typed renderer registry (replaces old Arc<dyn Any> system)
    pub renderers: crate::renderers_typed::TypedRendererRegistry,
}

impl EngineContext {
    /// Create a new engine context
    pub fn new() -> Self {
        Self {
            windows: Arc::new(DashMap::new()),
            project: Arc::new(RwLock::new(None)),
            launch: Arc::new(RwLock::new(LaunchContext::new())),
            discord: Arc::new(RwLock::new(None)),
            multiuser: Arc::new(RwLock::new(None)),
            type_database: Arc::new(RwLock::new(None)),
            window_sender: Arc::new(RwLock::new(None)),
            window_count: Arc::new(parking_lot::Mutex::new(0)),
            renderers: crate::renderers_typed::TypedRendererRegistry::new(),
        }
    }

    /// Set window request sender (builder pattern)
    pub fn with_window_sender(self, sender: WindowRequestSender) -> Self {
        *self.window_sender.write() = Some(sender);
        self
    }

    /// Request a new window
    pub fn request_window(&self, request: WindowRequest) {
        let sender = self.window_sender.read();
        if let Some(sender) = sender.as_ref() {
            if let Err(e) = sender.send(request) {
                tracing::error!("Failed to send window request: {}", e);
            }
        } else {
            tracing::error!("Window sender not initialized!");
        }
    }

    /// Register a window context
    pub fn register_window(&self, window_id: WindowId, context: WindowContext) {
        self.windows.insert(window_id, context);
        *self.window_count.lock() += 1;
    }

    /// Unregister a window
    pub fn unregister_window(&self, window_id: &WindowId) -> Option<WindowContext> {
        let result = self.windows.remove(window_id).map(|(_, ctx)| ctx);
        if result.is_some() {
            let mut count = self.window_count.lock();
            *count = count.saturating_sub(1);
        }
        result
    }

    /// Get window count
    pub fn window_count(&self) -> usize {
        *self.window_count.lock()
    }

    /// Set current project
    pub fn set_project(&self, project: ProjectContext) {
        *self.project.write() = Some(project);
    }

    /// Clear current project
    pub fn clear_project(&self) {
        *self.project.write() = None;
    }

    /// Initialize Discord Rich Presence
    pub fn init_discord(&self, application_id: impl Into<String>) -> anyhow::Result<()> {
        let presence = DiscordPresence::new(application_id);
        presence.connect()?;
        *self.discord.write() = Some(presence);
        Ok(())
    }

    /// Get Discord presence handle
    pub fn discord(&self) -> Option<DiscordPresence> {
        self.discord.read().clone()
    }

    /// Update Discord presence
    pub fn update_discord_presence(
        &self,
        project_name: Option<String>,
        tab_name: Option<String>,
        file_path: Option<String>,
    ) {
        if let Some(discord) = self.discord.read().as_ref() {
            discord.update_all(project_name, tab_name, file_path);
        }
    }

    /// Set global type database
    pub fn set_type_database(&self, type_database: Arc<TypeDatabase>) {
        *self.type_database.write() = Some(type_database);
    }

    /// Get global type database
    pub fn type_database(&self) -> Option<Arc<TypeDatabase>> {
        self.type_database.read().clone()
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
        *self.multiuser.write() = Some(context.clone());
        // Also update the global static for backward compatibility
        crate::multiuser::set_multiuser_context(context);
    }

    /// Clear multiuser session context
    ///
    /// Call this when disconnecting from a session.
    pub fn clear_multiuser(&self) {
        *self.multiuser.write() = None;
        crate::multiuser::clear_multiuser_context();
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
    pub fn are_we_multiuser_host(&self) -> bool {
        self.multiuser.read()
            .as_ref()
            .map(|ctx| ctx.is_host)
            .unwrap_or(false)
    }

    /// Update multiuser connection status
    pub fn set_multiuser_status(&self, status: crate::multiuser::MultiuserStatus) {
        if let Some(ctx) = self.multiuser.write().as_mut() {
            ctx.set_status(status.clone());
            // Sync to global static
            crate::multiuser::set_multiuser_status(status);
        }
    }

    /// Add a participant to the current multiuser session
    pub fn add_multiuser_participant(&self, peer_id: impl Into<String>) {
        let peer_id = peer_id.into();
        if let Some(ctx) = self.multiuser.write().as_mut() {
            ctx.add_participant(peer_id.clone());
            // Sync to global static
            crate::multiuser::add_participant(peer_id);
        }
    }

    /// Remove a participant from the current multiuser session
    pub fn remove_multiuser_participant(&self, peer_id: &str) {
        if let Some(ctx) = self.multiuser.write().as_mut() {
            ctx.remove_participant(peer_id);
            // Sync to global static
            crate::multiuser::remove_participant(peer_id);
        }
    }

    /// Legacy metadata setter (for backward compatibility)
    ///
    /// **Deprecated**: Use typed context fields instead.
    /// This method exists only for compatibility during migration.
    #[deprecated(since = "0.2.0", note = "Use typed context fields instead (e.g., engine_context.project.write())")]
    pub fn set_metadata(&self, _key: String, _value: String) {
        // No-op: metadata system has been replaced with typed contexts
        // Calling code should be migrated to use typed fields
    }

    /// Set as global instance (for GPUI views that need global access)
    pub fn set_global(self) {
        GLOBAL_CONTEXT.set(self).ok();
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

use std::sync::OnceLock;
static GLOBAL_CONTEXT: OnceLock<EngineContext> = OnceLock::new();

/// Migration helpers for transitioning from EngineState metadata to EngineContext
///
/// These provide a compatibility layer during the migration period.
pub mod migration {
    use super::*;

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
        let window_id: WindowId = 42;
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
        assert_eq!(launch.verbose, true);
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

        assert!(context.project.read().is_none());

        let project = ProjectContext::new(PathBuf::from("/test"));
        context.set_project(project.clone());

        assert!(context.project.read().is_some());
        assert_eq!(context.project.read().as_ref().unwrap().path, PathBuf::from("/test"));

        context.clear_project();
        assert!(context.project.read().is_none());
    }
}
