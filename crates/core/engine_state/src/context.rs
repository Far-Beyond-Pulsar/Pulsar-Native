//! Typed Engine Context System
//!
//! Replaces the string-based metadata system with type-safe context objects.
//! Each context represents a specific domain (windows, projects, etc.) with
//! proper types instead of string key-value pairs.

use crate::DiscordPresence;
use dashmap::DashMap;
use pulsar_auth::AuthProfile;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use ui_types_common::window_types::{WindowId, WindowRequest};

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

/// What a new project starts with, given by the host that embeds it (the
/// editor binary): the default level, and the project assets it references
/// (paths relative to the project's `assets/` directory). The editor writes
/// it into a project that has no level yet; it names nothing of what is in
/// it, so the content (its kinds' components and files) is the host's.
#[derive(Clone, Debug, Default)]
pub struct NewProjectContent {
    /// The default level's bytes; `None` starts new projects empty.
    pub level: Option<std::sync::Arc<[u8]>>,
    /// `(path under assets/, bytes)` for every asset the level references.
    pub assets: Vec<(String, std::sync::Arc<[u8]>)>,
}

impl NewProjectContent {
    /// Write the assets into `<project_root>/assets/`, leaving any file the
    /// project already has untouched. A file that cannot be written is
    /// logged and skipped.
    pub fn install_assets(&self, project_root: &std::path::Path) {
        for (path, bytes) in &self.assets {
            let dst = project_root.join("assets").join(path);
            if engine_fs::virtual_fs::exists(&dst).unwrap_or(false) {
                continue;
            }
            if let Some(parent) = dst.parent() {
                if let Err(error) = engine_fs::virtual_fs::create_dir_all(parent) {
                    tracing::warn!("Could not create {}: {error}", parent.display());
                    continue;
                }
            }
            match engine_fs::virtual_fs::write_file(&dst, bytes) {
                Ok(()) => tracing::debug!("Starter asset written: {}", dst.display()),
                Err(error) => tracing::warn!("Could not write {}: {error}", dst.display()),
            }
        }
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
            window_state: crate::keyed_store::KeyedStore::new(),
            store,
        }
    }

    /// Unregister a window
    pub fn unregister_window(&self, window_id: &WindowId) -> Option<WindowContext> {
        self.windows.remove(window_id).map(|(_, ctx)| ctx)
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

    /// Set authenticated user profile.
    pub fn set_auth_profile(&self, profile: AuthProfile) {
        self.store
            .get_or_init::<Option<AuthProfile>>()
            .set(Some(profile));
    }

    /// Clear authenticated user profile.
    pub fn clear_auth_profile(&self) {
        self.store.get_or_init::<Option<AuthProfile>>().set(None);
    }

    /// Get authenticated user profile.
    pub fn auth_profile(&self) -> Option<AuthProfile> {
        self.store
            .get_or_init::<Option<AuthProfile>>()
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

    /// Notify listeners that the multiuser snapshot changed.
    pub fn notify_multiuser_changed(&self) {
        self.multiuser.update(|_| {});
    }

    /// Set as global instance (for GPUI views that need global access). The
    /// first context set stays the global one; a later call is reported.
    pub fn set_global(self) {
        if GLOBAL_CONTEXT.set(self).is_err() {
            tracing::warn!(
                "EngineContext::set_global called again; the first context stays global"
            );
        }
    }

    /// Get global instance: this process's engine context. In a plugin library it is the
    /// host's, bound by [`attach_exported`] before the plugin runs any code (the library's
    /// own copy of this crate never has a context of its own).
    pub fn global() -> Option<&'static Self> {
        GLOBAL_CONTEXT.get().or_else(|| HOST_CONTEXT.get().copied())
    }
}

impl Default for EngineContext {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_CONTEXT: OnceLock<EngineContext> = OnceLock::new();

/// The host's context, in a plugin library's copy of this crate ([`attach_exported`]). Every plugin
/// library links its own copy of this crate, whose [`GLOBAL_CONTEXT`] nothing sets; without this
/// binding the project path, the engine-wide state store and every window the host opened read as
/// absent inside the library, and a component that resolves its assets against the project (a
/// mesh's `.mesh` file) loads nothing.
static HOST_CONTEXT: OnceLock<&'static EngineContext> = OnceLock::new();

/// The entry point a plugin library exports for [`attach_exported`]
/// ([`export_engine_state_attach!`](crate::export_engine_state_attach), which
/// `plugin_editor_api::export_plugin!` invokes).
pub const ATTACH_SYMBOL: &str = "_plugin_attach_engine_state";

/// Bumped on any change to [`HostContext`] or to how a copy reads it.
const ATTACH_VERSION: u64 = 1;

/// The layout this copy of the crate was built with: a library built from other sources of this crate
/// refuses the host's context instead of reading it as the wrong type.
const fn layout() -> u64 {
    let parts = [
        ATTACH_VERSION as usize,
        size_of::<HostContext>(),
        size_of::<EngineContext>(),
        size_of::<ProjectContext>(),
        size_of::<WindowContext>(),
        size_of::<crate::StateStore>(),
    ];
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut index = 0;
    while index < parts.len() {
        hash ^= parts[index] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 1;
    }
    hash
}

/// What the host hands a plugin library: its engine context, with the layout it was built with.
#[repr(C)]
pub struct HostContext {
    layout: u64,
    context: *const EngineContext,
}

// SAFETY: `context` points to the process's global `EngineContext`, a `static` that is never moved or
// dropped and is only read through shared references (it is `Sync`: its state is behind locks).
unsafe impl Send for HostContext {}
// SAFETY: as above.
unsafe impl Sync for HostContext {}

/// The host's engine context for a plugin library, or null before [`EngineContext::set_global`] ran.
/// It lives for the process (plugins are never unloaded).
pub fn export_host_context() -> *const std::ffi::c_void {
    static EXPORTED: OnceLock<HostContext> = OnceLock::new();
    let Some(context) = GLOBAL_CONTEXT.get() else {
        return std::ptr::null();
    };
    let exported = EXPORTED.get_or_init(|| HostContext {
        layout: layout(),
        context: std::ptr::from_ref(context),
    });
    (exported as *const HostContext).cast()
}

/// Bind this copy of the crate (a plugin library's) to the host's engine context, so
/// [`EngineContext::global`] answers inside the library. `false` when `host` is null or was built with
/// another layout.
///
/// # Safety
///
/// `host` is null or a pointer from the host's [`export_host_context`], alive for the process.
#[doc(hidden)]
pub unsafe fn attach_exported(host: *const std::ffi::c_void) -> bool {
    // SAFETY: the caller's contract.
    let Some(host) = (unsafe { host.cast::<HostContext>().as_ref() }) else {
        return false;
    };
    if host.layout != layout() || host.context.is_null() {
        tracing::error!("refused the host's engine context: built with another layout");
        return false;
    }
    // SAFETY: the host's context lives for the process, and the layout matches this copy's.
    let _ = HOST_CONTEXT.set(unsafe { &*host.context });
    true
}

/// Export [`ATTACH_SYMBOL`] from a plugin library, so the editor's plugin manager binds the library's
/// copy of this crate to the editor's engine context before the plugin runs any code.
/// `plugin_editor_api::export_plugin!` invokes it.
#[macro_export]
macro_rules! export_engine_state_attach {
    () => {
        /// Bind this library's copy of `engine_state` to the host's engine context.
        /// Called by the plugin manager before `_plugin_create`.
        ///
        /// # Safety
        ///
        /// `host` is the host's exported engine context, alive for the process.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_attach_engine_state(
            host: *const ::std::ffi::c_void,
        ) -> bool {
            // A panic must not unwind across the FFI boundary: the host reads it as a refusal.
            ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
                // SAFETY: the caller's contract.
                unsafe { $crate::context::attach_exported(host) }
            }))
            .unwrap_or(false)
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plugin library's copy of this crate has no context of its own (nothing calls `set_global`
    /// in it): after the host's is attached, the project path reads as the host's. Without it every
    /// component that resolves its assets against the project (a mesh) found no project at all.
    #[test]
    fn an_attached_host_context_answers_in_a_copy_with_none_of_its_own() {
        assert!(
            GLOBAL_CONTEXT.get().is_none(),
            "this test stands for a copy that has no context of its own"
        );
        assert!(EngineContext::global().is_none());
        assert_eq!(crate::get_project_path(), None);

        // A null pointer and another layout are refused, and bind nothing.
        // SAFETY: null is allowed by the contract.
        assert!(!unsafe { attach_exported(std::ptr::null()) });
        let mut host = EngineContext::new();
        host.set_project(ProjectContext::new(PathBuf::from("/host/project")));
        let host: &'static EngineContext = Box::leak(Box::new(host));
        let wrong = HostContext {
            layout: layout() ^ 1,
            context: std::ptr::from_ref(host),
        };
        // SAFETY: `wrong` is a live `HostContext`.
        assert!(!unsafe { attach_exported(std::ptr::from_ref(&wrong).cast()) });
        assert!(EngineContext::global().is_none());

        let right = HostContext {
            layout: layout(),
            context: std::ptr::from_ref(host),
        };
        // SAFETY: `right` is a live `HostContext` of this copy's layout.
        assert!(unsafe { attach_exported(std::ptr::from_ref(&right).cast()) });
        assert!(EngineContext::global().is_some());
        assert_eq!(
            crate::get_project_path().as_deref(),
            Some(PathBuf::from("/host/project").to_string_lossy().as_ref())
        );
    }

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
        assert_eq!(context.windows.len(), 0);

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

        project_handle.set(None);
        assert!(project_handle.read().is_none());
    }
}
