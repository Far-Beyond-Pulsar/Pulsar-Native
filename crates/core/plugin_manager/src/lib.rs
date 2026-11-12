//! # Plugin Manager
//!
//! This crate provides the infrastructure for loading, managing, and using editor plugins
//! in the Pulsar engine. It handles:
//!
//! - Dynamic library loading from `plugins/editor/`
//! - Version compatibility checking
//! - File type and editor registration
//! - Editor instance creation
//!
//! ## Where to look
//!
//! [`PluginManager`] is defined here; its methods are grouped by topic, one
//! module per group (each starts with a doc saying where its entry points are):
//!
//! | Module | Owns |
//! |---|---|
//! | `loading` | prepare / install / order plugin libraries |
//! | `attach` | sharing host runtimes with a library through its exported symbols |
//! | `details` | plugin listings for the plugin manager window |
//! | `catalog` | installed plugins and their linked contributions |
//! | `contributions` | built-in registration, statusbar, scripts, components, `drain_*` |
//! | `editors` | creating editors and new files |
//! | `ai_tools` | AI tool bridges and tool execution |
//! | `global` | the process-wide manager and lock-free file-type lookup |
//! | `startup` | splash-screen prewarm, staging and preloads |
//! | `error` | [`PluginManagerError`] |
//! | `registry`, `restorers`, `builtin`, `dependencies`, `tool_bridge`, `permanent_library` | supporting types |
//!
//! ## Safety Model
//!
//! This plugin system eliminates undefined behavior through **permanent library loading**:
//!
//! - Plugins are loaded once at startup and NEVER unloaded
//! - Uses `PermanentLibrary` wrapper to prevent `dlclose`/`FreeLibrary`
//! - All function pointers, vtables, and drop glue remain valid for process lifetime
//! - Safe to share `Arc<T>`, trait objects, and function pointers across boundary
//!
//! ## Usage
//!
//! ```rust,ignore
//! use plugin_manager::PluginManager;
//!
//! // Create and initialize the plugin manager
//! let mut manager = PluginManager::new();
//!
//! // Load plugins once at startup
//! manager.load_plugins_from_dir("plugins/editor", &cx)?;
//!
//! // Query available file types
//! let file_types = manager.file_type_registry().get_all_file_types();
//!
//! // Create an editor for a file
//! let editor = manager.create_editor_for_file(&file_path, window, cx)?;
//!
//! // Use the editor
//! workspace.add_tab(editor);
//!
//! // That's it! No unloading, no cleanup needed.
//! // Plugins stay loaded until process exits.
//! ```
//!
//! ## Memory Management
//!
//! Because plugins are never unloaded, memory management is simple:
//!
//! - Plugins can return `Arc<T>` directly (no weak reference workarounds)
//! - Drop glue is always valid (plugin code never unmaps)
//! - Trait objects work normally (vtables never invalidate)
//! - Function pointers can be stored indefinitely
//!
//! The only concern is **Arc cycles**, which can cause memory leaks.
//! See `PLUGIN_ARCHITECTURE.md` for guidelines on preventing cycles with `Weak<T>`.

#![warn(clippy::undocumented_unsafe_blocks)]

use parking_lot::RwLock;
use plugin_editor_api::*;
use std::collections::HashMap;
use std::path::PathBuf;

mod ai_tools;
mod attach;
pub mod builtin;
pub mod catalog;
mod contributions;
pub mod dependencies;
mod details;
mod editors;
mod error;
mod global;
mod loading;
mod panel_decoration;
mod permanent_library;
mod registry;
mod restorers;
mod startup;
pub mod tool_bridge;

/// Re-exported so a linked provider can `inventory::submit!` without its own
/// dependency (see [`PluginEditorProvider`]).
pub use ::inventory;
pub use attach::{attach_event_bus, attach_profiler, attach_world_runtime, read_manifest};
pub use builtin::{
    BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext, PluginEditorProvider,
};
pub use catalog::{
    CatalogPlugin, CatalogProblem, CatalogSnapshot, ContractLink, Provider, Requirement,
    RequirementState,
};
pub use dependencies::{plan_load_order, DependencyError, LoadPlan};
pub use error::PluginManagerError;
pub use global::{file_type_for_path, global, initialize_global};
pub use permanent_library::{IntegrityError, PermanentLibrary, PermanentLibraryLoadError};
pub use registry::{EditorRegistry, FileTypeLookup, FileTypeRegistry};
pub use restorers::{panel_restorers, PanelRestorers, RestoreOutcome};
pub use startup::{
    prewarm_plugins, publish_preload_targets, run_plugin_preloads, stage_manager,
    take_prewarmed_plugins, take_staged_manager, PrewarmedPlugins,
};
pub use tool_bridge::PluginToolBridge;

// ============================================================================
// Plugin Container
// ============================================================================

/// A loaded plugin with its library handle.
///
/// # Memory Model
///
/// Because plugins are never unloaded:
/// - The `plugin` reference has `'static` lifetime (safe because library never unloads)
/// - The `library` handle is wrapped in `PermanentLibrary` (prevents dlclose/FreeLibrary)
/// - All plugin code, vtables, and drop glue remain valid forever
/// A plugin library that is loaded and verified but has not run any of its own
/// code yet; see [`PluginManager::prepare_plugin`].
pub struct PreparedPlugin {
    path: PathBuf,
    library: PermanentLibrary,
}

/// One dependency of a plugin and whether it is met right now.
#[derive(Debug, Clone)]
pub struct DependencyStatus {
    pub dependency: PluginDependency,
    pub optional: bool,
    pub state: DependencyState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyState {
    Satisfied { version: String },
    VersionMismatch { found: String },
    Missing,
}

/// A loaded plugin library, as the catalog shows it; see
/// [`PluginManager::library_details`].
#[derive(Debug, Clone)]
pub struct PluginDetails {
    pub metadata: PluginMetadata,
    pub library_path: PathBuf,
    pub dependencies: Vec<DependencyStatus>,
    /// Loaded plugins that list this one in `requires` or `optional`.
    pub required_by: Vec<PluginId>,
    pub file_types: Vec<String>,
    pub editors: Vec<String>,
    pub components: Vec<String>,
    pub statusbar_buttons: Vec<String>,
    pub ai_tools: Vec<String>,
}

struct LoadedPlugin {
    /// Reference to the plugin instance.
    ///
    /// SAFETY: This has 'static lifetime because:
    /// 1. The plugin library is never unloaded (PermanentLibrary prevents it)
    /// 2. The plugin is created by leaking a Box (intentional permanent allocation)
    /// 3. The reference remains valid for the process lifetime
    plugin: &'static dyn EditorPluginFull,

    /// The dynamic library handle (must be kept alive).
    ///
    /// SAFETY: PermanentLibrary ensures this is never unloaded.
    /// As long as this exists, all symbols from the library remain valid.
    #[allow(dead_code)]
    library: PermanentLibrary,

    /// Metadata for quick access (owned by main app)
    metadata: PluginMetadata,

    /// Editor factories registered by this plugin (populated at load time).
    editor_factories: EditorFactoryRegistry,
}

// ============================================================================
// Plugin Manager
// ============================================================================

/// Manages all editor plugins in the system.
///
/// The PluginManager is responsible for:
/// - Loading plugins from disk (once, at startup)
/// - Verifying version compatibility
/// - Maintaining registries of file types and editors
/// - Creating editor instances on demand
/// - Managing built-in editors (same trait interface, no DLL loading)
/// - Managing statusbar buttons registered by plugins
///
/// # Safety
///
/// The PluginManager uses permanent library loading to eliminate undefined behavior:
/// - Libraries are loaded once and never unloaded
/// - All plugin code remains valid for process lifetime
/// - Safe to share Arc<T> and trait objects across boundary
///
/// # Thread Safety
///
/// The global plugin manager is wrapped in `RwLock` for thread-safe access.
/// Individual plugin calls are synchronized through this lock.
pub struct PluginManager {
    /// All loaded plugins, indexed by plugin ID
    plugins: HashMap<PluginId, LoadedPlugin>,

    /// Registry of all file types
    file_type_registry: FileTypeRegistry,

    /// Registry of all editors
    editor_registry: EditorRegistry,

    /// Built-in editor registry (no DLL loading)
    builtin_registry: BuiltinEditorRegistry,

    /// The version info for this engine build
    engine_version: VersionInfo,

    /// Project root path for editor context
    project_root: Option<PathBuf>,

    /// Host-owned editor settings passed to plugin editor factories.
    editor_settings: EditorSettingsSnapshot,
    /// Engine services (the language server) handed to every editor created.
    /// Held weakly: this manager is a process-wide static and must not keep
    /// the app's entities alive past the app.
    editor_services: WeakEditorServices,

    /// Statusbar buttons registered by all plugins
    /// Stored with plugin ownership tracking for proper cleanup
    statusbar_buttons: Vec<(PluginId, StatusbarButtonDefinition)>,

    /// Component factories from plugins, collected at load time.
    /// Each entry is (class_name, factory).
    plugin_component_registrations: Vec<(String, plugin_editor_api::ComponentFactory)>,
}

// SAFETY: PluginManager now contains only safe types:
// - &'static dyn EditorPluginFull (safe because plugin never unloads)
// - PermanentLibrary (safe wrapper that prevents unload)
// - Normal Rust collections (HashMap, Vec, etc.)
//
// The RwLock wrapper in the global instance provides thread safety.
unsafe impl Send for PluginManager {}
// SAFETY: as for `Send`: shared access goes through `&self` methods over the
// same plain collections, and the plugin objects are only reached under the
// global instance's `RwLock`.
unsafe impl Sync for PluginManager {}

impl PluginManager {
    /// Create a new plugin manager.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            file_type_registry: FileTypeRegistry::new(),
            editor_registry: EditorRegistry::new(),
            builtin_registry: BuiltinEditorRegistry::new(),
            engine_version: VersionInfo::current(),
            project_root: None,
            editor_settings: EditorSettingsSnapshot::default(),
            editor_services: WeakEditorServices::default(),
            statusbar_buttons: Vec::new(),
            plugin_component_registrations: Vec::new(),
        }
    }

    /// Set the project root path for editor context.
    pub fn set_project_root(&mut self, project_root: Option<PathBuf>) {
        self.project_root = project_root;
    }

    /// Replace the host-owned settings passed to subsequently created plugin editors.
    pub fn set_editor_settings_snapshot(&mut self, settings: EditorSettingsSnapshot) {
        self.editor_settings = settings;
    }

    /// Sets the engine services every editor receives. Kept apart from the
    /// settings snapshot, which the host replaces wholesale on every change.
    pub fn set_editor_services(&mut self, services: EditorServices) {
        self.editor_services = services.downgrade();
    }

    /// The services editors are created with: those the host set, while the
    /// host keeps them alive.
    pub fn editor_services(&self) -> EditorServices {
        self.editor_services.upgrade()
    }

    /// The settings snapshot a dynamic plugin's factory receives: the host's
    /// settings plus the engine services.
    pub fn snapshot_for_editor(&self) -> EditorSettingsSnapshot {
        self.editor_settings
            .clone()
            .with_services(self.editor_services())
    }

    /// Get a reference to the file type registry.
    pub fn file_type_registry(&self) -> &FileTypeRegistry {
        &self.file_type_registry
    }

    /// Get a reference to the editor registry.
    pub fn editor_registry(&self) -> &EditorRegistry {
        &self.editor_registry
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

// When the manager is dropped, we DON'T destroy plugins (they stay loaded forever)
// We just log that we're shutting down
impl Drop for PluginManager {
    fn drop(&mut self) {
        tracing::info!(
            "Plugin manager shutting down ({} plugins loaded)",
            self.plugins.len()
        );

        // Note: We intentionally do NOT unload plugins or call destroy functions.
        // Plugins remain loaded until process termination. This is safe and intentional.
        //
        // The OS will clean up all plugin memory when the process exits.
    }
}
