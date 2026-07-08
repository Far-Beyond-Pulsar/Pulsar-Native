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

use once_cell::sync::OnceCell;
use parking_lot::RwLock;
use plugin_editor_api::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dock::PanelView;

struct FileTypeDecoratedPanelView {
    inner: Arc<dyn PanelView>,
    file_path: PathBuf,
    icon: Option<ui::IconName>,
}

impl PanelView for FileTypeDecoratedPanelView {
    fn panel_name(&self, cx: &gpui::App) -> &'static str {
        self.inner.panel_name(cx)
    }

    fn panel_id(&self, cx: &gpui::App) -> gpui::EntityId {
        self.inner.panel_id(cx)
    }

    fn tab_name(&self, cx: &gpui::App) -> Option<gpui::SharedString> {
        self.inner.tab_name(cx)
    }

    fn tab_icon(&self, cx: &gpui::App) -> Option<ui::IconName> {
        self.inner.tab_icon(cx).or_else(|| self.icon.clone())
    }

    fn tab_unsaved(&self, cx: &gpui::App) -> bool {
        self.inner.tab_unsaved(cx)
    }

    fn panel_file_path(&self, cx: &gpui::App) -> Option<PathBuf> {
        self.inner.panel_file_path(cx).or_else(|| {
            if self.file_path.as_os_str().is_empty() {
                None
            } else {
                Some(self.file_path.clone())
            }
        })
    }

    fn title(&self, window: &gpui::Window, cx: &gpui::App) -> gpui::AnyElement {
        self.inner.title(window, cx)
    }

    fn title_suffix(
        &self,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<gpui::AnyElement> {
        self.inner.title_suffix(window, cx)
    }

    fn title_style(&self, cx: &gpui::App) -> Option<ui::dock::TitleStyle> {
        self.inner.title_style(cx)
    }

    fn closable(&self, cx: &gpui::App) -> bool {
        self.inner.closable(cx)
    }

    fn zoomable(&self, cx: &gpui::App) -> Option<ui::dock::PanelControl> {
        self.inner.zoomable(cx)
    }

    fn visible(&self, cx: &gpui::App) -> bool {
        self.inner.visible(cx)
    }

    fn set_active(&self, active: bool, window: &mut gpui::Window, cx: &mut gpui::App) {
        self.inner.set_active(active, window, cx)
    }

    fn set_zoomed(&self, zoomed: bool, window: &mut gpui::Window, cx: &mut gpui::App) {
        self.inner.set_zoomed(zoomed, window, cx)
    }

    fn popup_menu(
        &self,
        menu: ui::popup_menu::PopupMenu,
        window: &gpui::Window,
        cx: &gpui::App,
    ) -> ui::popup_menu::PopupMenu {
        self.inner.popup_menu(menu, window, cx)
    }

    fn toolbar_buttons(
        &self,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<Vec<ui::button::Button>> {
        self.inner.toolbar_buttons(window, cx)
    }

    fn view(&self) -> gpui::AnyView {
        self.inner.view()
    }

    fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.inner.focus_handle(cx)
    }

    fn dump(&self, cx: &gpui::App) -> ui::dock::PanelState {
        self.inner.dump(cx)
    }

    fn inner_padding(&self, cx: &gpui::App) -> bool {
        self.inner.inner_padding(cx)
    }

    fn discord_icon_key(&self, cx: &gpui::App) -> &'static str {
        self.inner.discord_icon_key(cx)
    }
}

pub mod builtin;
mod permanent_library;
mod registry;
pub mod tool_bridge;

pub use builtin::{BuiltinEditorProvider, BuiltinEditorRegistry, EditorContext};
pub use permanent_library::{IntegrityError, PermanentLibrary};
pub use registry::{EditorRegistry, FileTypeRegistry};
pub use tool_bridge::PluginToolBridge;

// ============================================================================
// Global Plugin Manager
// ============================================================================

/// Global plugin manager instance.
static GLOBAL_PLUGIN_MANAGER: OnceCell<RwLock<PluginManager>> = OnceCell::new();

/// Initialize the global plugin manager.
/// This should be called once at application startup.
pub fn initialize_global(manager: PluginManager) {
    if GLOBAL_PLUGIN_MANAGER.set(RwLock::new(manager)).is_err() {
        tracing::warn!("Global plugin manager already initialized");
    }
}

/// Get a handle to the global plugin manager.
///
/// `parking_lot`-backed — `.read()` / `.write()` return guards directly,
/// no `.unwrap()` needed.
pub fn global() -> Option<&'static RwLock<PluginManager>> {
    GLOBAL_PLUGIN_MANAGER.get()
}

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

    /// Statusbar buttons registered by all plugins
    /// Stored with plugin ownership tracking for proper cleanup
    statusbar_buttons: Vec<(PluginId, StatusbarButtonDefinition)>,

    /// Subsystems provided by plugins, collected at load time.
    /// Merged into the engine's SubsystemRegistry at startup.
    plugin_subsystems: Vec<Box<dyn engine_subsystems::Subsystem>>,

    /// Component factories from plugins, collected at load time.
    /// Each entry is (class_name, factory).
    plugin_component_registrations: Vec<(
        String,
        plugin_editor_api::ComponentFactory,
    )>,

    /// Component definitions registered directly as built-ins (not from DLL plugins).
    /// These supplement definitions from `BuiltinEditorRegistry` and DLL plugins.
    builtin_component_definitions: Vec<ComponentDefinition>,
}

// SAFETY: PluginManager now contains only safe types:
// - &'static dyn EditorPluginFull (safe because plugin never unloads)
// - PermanentLibrary (safe wrapper that prevents unload)
// - Normal Rust collections (HashMap, Vec, etc.)
//
// The RwLock wrapper in the global instance provides thread safety.
unsafe impl Send for PluginManager {}
unsafe impl Sync for PluginManager {}

impl PluginManager {
    fn decorate_editor_panel_for_path(
        &self,
        panel: Arc<dyn PanelView>,
        file_path: &Path,
    ) -> Arc<dyn PanelView> {
        let icon = self
            .get_file_type_for_path(file_path)
            .map(|ft| ft.icon.clone());

        if icon.is_none() && file_path.as_os_str().is_empty() {
            panel
        } else {
            Arc::new(FileTypeDecoratedPanelView {
                inner: panel,
                file_path: file_path.to_path_buf(),
                icon,
            })
        }
    }

    /// Create a new plugin manager.
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
            file_type_registry: FileTypeRegistry::new(),
            editor_registry: EditorRegistry::new(),
            builtin_registry: BuiltinEditorRegistry::new(),
            engine_version: VersionInfo::current(),
            project_root: None,
            statusbar_buttons: Vec::new(),
            plugin_subsystems: Vec::new(),
            plugin_component_registrations: Vec::new(),
            builtin_component_definitions: Vec::new(),
        }
    }

    /// Set the project root path for editor context.
    pub fn set_project_root(&mut self, project_root: Option<PathBuf>) {
        self.project_root = project_root;
    }

    /// Get a mutable reference to the built-in editor registry.
    ///
    /// This allows external code to register built-in editors during initialization.
    pub fn builtin_registry_mut(&mut self) -> &mut BuiltinEditorRegistry {
        &mut self.builtin_registry
    }

    /// Register all built-in editors with the file type and editor registries.
    ///
    /// This should be called after all built-in editors have been registered
    /// with the builtin registry.
    pub fn register_builtin_editors(&mut self) {
        self.builtin_registry
            .register_all(&mut self.file_type_registry, &mut self.editor_registry);
    }

    /// Register a built-in subsystem that will be drained and injected alongside
    /// plugin subsystems.
    pub fn register_builtin_subsystem(&mut self, subsystem: Box<dyn engine_subsystems::Subsystem>) {
        self.plugin_subsystems.push(subsystem);
    }

    /// Register built-in component definitions.
    pub fn register_builtin_component_definitions(&mut self, defs: Vec<ComponentDefinition>) {
        self.builtin_component_definitions.extend(defs);
    }

    /// Register built-in component factories that will be drained and injected
    /// alongside plugin component factories.
    pub fn register_builtin_component_factories(
        &mut self,
        factories: Vec<(String, plugin_editor_api::ComponentFactory)>,
    ) {
        self.plugin_component_registrations.extend(factories);
    }

    /// Load all plugins from a directory.
    ///
    /// This will scan the directory for dynamic libraries (.dll on Windows,
    /// .so on Linux, .dylib on macOS) and attempt to load each one as a plugin.
    ///
    /// Plugins that fail version checks or loading will be logged but won't
    /// prevent other plugins from loading.
    ///
    /// # Important
    ///
    /// Plugins are loaded ONCE and NEVER unloaded. This is intentional and
    /// necessary for safety. See `PLUGIN_ARCHITECTURE.md` for details.
    pub fn load_plugins_from_dir(
        &mut self,
        dir: impl AsRef<Path>,
        cx: &gpui::App,
    ) -> Result<(), PluginManagerError> {
        let dir = dir.as_ref();

        if !dir.exists() {
            tracing::warn!("Plugin directory does not exist: {:?}", dir);
            return Ok(());
        }

        tracing::info!("Loading plugins from: {:?}", dir);

        // Get the appropriate file extension for this platform
        #[cfg(target_os = "windows")]
        let extension = "dll";
        #[cfg(target_os = "linux")]
        let extension = "so";
        #[cfg(target_os = "macos")]
        let extension = "dylib";

        // Scan directory for plugin libraries
        for entry in walkdir::WalkDir::new(dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();

            // Check if this is a dynamic library
            if path.extension().and_then(|s| s.to_str()) != Some(extension) {
                continue;
            }

            // Attempt to load the plugin
            match self.load_plugin(path, cx) {
                Ok(plugin_id) => {
                    tracing::info!("✅ Successfully loaded plugin: {}", plugin_id);
                }
                Err(e) => {
                    tracing::error!("❌ Failed to load plugin from {:?}: {}", path, e);
                }
            }
        }

        Ok(())
    }

    /// Load a single plugin from a library file.
    ///
    /// # Safety
    ///
    /// This function loads and executes code from a dynamic library. The library
    /// must be compiled with the same Rust version and for the same engine version
    /// as the current build. These are checked at runtime.
    ///
    /// The loaded library will NEVER be unloaded. This is intentional and necessary
    /// to prevent undefined behavior.
    ///
    /// # Returns
    ///
    /// Returns the `PluginId` of the loaded plugin on success.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The library file cannot be loaded
    /// - Required symbols are missing
    /// - Version compatibility check fails
    /// - Plugin creation fails
    pub fn load_plugin(
        &mut self,
        path: impl AsRef<Path>,
        cx: &gpui::App,
    ) -> Result<PluginId, PluginManagerError> {
        let path = path.as_ref();

        tracing::debug!("Loading plugin from: {:?}", path);

        // Load the library permanently
        let library =
            PermanentLibrary::new(path).map_err(|e| PluginManagerError::LibraryLoadError {
                path: path.to_path_buf(),
                message: e.to_string(),
            })?;

        // Verify the plugin binary against the required integrity manifest.
        // The manifest is a sibling JSON file named "plugin_integrity.json" that
        // maps plugin filenames to their expected SHA-256 hex digests.
        // If no manifest exists, plugins are rejected (manifest is mandatory).
        let plugin_dir = path.parent().unwrap_or(Path::new("."));
        let manifest = Self::load_plugin_manifest(plugin_dir)?;
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let expected_hex = manifest.get(file_name);
        if let Some(expected_hex) = expected_hex {
            let expected: [u8; 32] = Self::parse_hex_hash(expected_hex).map_err(|e| {
                PluginManagerError::LibraryLoadError {
                    path: path.to_path_buf(),
                    message: format!("Invalid hash in manifest for '{}': {}", file_name, e),
                }
            })?;
            if library.sha256() != &expected {
                return Err(PluginManagerError::IntegrityCheckFailed {
                    path: path.to_path_buf(),
                    expected: expected,
                    actual: *library.sha256(),
                });
            }
            tracing::debug!("Integrity check passed for plugin: {:?}", path);
        } else {
            // Plugin not listed in manifest — reject unless manifest explicitly
            // allows unlisted plugins via the special "allow_unlisted": true key.
            if !manifest
                .get("__allow_unlisted__")
                .map_or(false, |v| v == "true")
            {
                return Err(PluginManagerError::IntegrityCheckFailed {
                    path: path.to_path_buf(),
                    expected: [0u8; 32],
                    actual: *library.sha256(),
                });
            }
        }

        // Get the version info function
        let version_fn: libloading::Symbol<extern "C" fn() -> VersionInfo> = unsafe {
            // SAFETY: We're loading a symbol from the permanently loaded library.
            // The symbol must exist and have the correct signature.
            // If it doesn't, we'll get an error and reject the plugin.
            library
                .get(b"_plugin_version")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_version".to_string(),
                    message: e.to_string(),
                })?
        };

        // Check version compatibility
        let plugin_version = version_fn();

        tracing::debug!(
            "Version check - Engine: {:?}, Plugin: {:?}",
            self.engine_version,
            plugin_version
        );

        if !self.engine_version.is_compatible(&plugin_version) {
            tracing::error!(
                "Plugin version mismatch! Expected engine v{}.{}.{} (rustc hash {:#x}), got v{}.{}.{} (rustc hash {:#x})",
                self.engine_version.engine_version.0,
                self.engine_version.engine_version.1,
                self.engine_version.engine_version.2,
                self.engine_version.rustc_version_hash,
                plugin_version.engine_version.0,
                plugin_version.engine_version.1,
                plugin_version.engine_version.2,
                plugin_version.rustc_version_hash,
            );

            return Err(PluginManagerError::VersionMismatch {
                expected: self.engine_version,
                actual: plugin_version,
            });
        }

        tracing::debug!("✅ Version check passed for plugin at {:?}", path);

        // Get the plugin constructor
        let create_fn: libloading::Symbol<PluginCreate> = unsafe {
            // SAFETY: Loading symbol from permanently loaded library.
            // The returned function pointer remains valid forever because
            // the library is never unloaded.
            library
                .get(b"_plugin_create")
                .map_err(|e| PluginManagerError::MissingSymbol {
                    symbol: "_plugin_create".to_string(),
                    message: e.to_string(),
                })?
        };

        // Create the plugin instance with Theme pointer for cross-DLL global state sync
        let plugin = unsafe {
            // SAFETY: Calling the plugin constructor is safe because:
            // 1. We trust the plugin code (internal plugins only)
            // 2. We've verified version compatibility
            // 3. The returned &'static mut reference is valid because the library never unloads
            //
            // We pass the Theme pointer to sync global state across the DLL boundary.
            // The Theme must remain valid for the process lifetime (guaranteed by engine).
            let theme_ptr = ui::theme::Theme::global(cx) as *const _ as *const std::ffi::c_void;

            // Validate theme pointer before passing to plugin
            if theme_ptr.is_null() {
                return Err(PluginManagerError::PluginCreationFailed {
                    message: "Theme pointer is null (engine global state not initialized)"
                        .to_string(),
                });
            }

            create_fn(theme_ptr)
        };

        let plugin: &'static mut dyn EditorPluginFull = plugin;

        // Get plugin metadata
        let metadata = plugin.metadata();
        let plugin_id = metadata.id.clone();

        tracing::info!(
            "📦 Loaded plugin: {} v{} by {}",
            metadata.name,
            metadata.version,
            metadata.author
        );

        // Call on_load hook
        plugin.on_load();

        // After load-time initialization we keep only an immutable static plugin ref.
        let plugin: &'static dyn EditorPluginFull = plugin;

        // Register file types
        let file_types = plugin.file_types();
        for file_type in file_types {
            tracing::debug!(
                "  📄 Registering file type: {} (.{})",
                file_type.display_name,
                file_type.extension
            );
            self.file_type_registry
                .register(file_type, plugin_id.clone());
        }

        // Register editors
        let editors = plugin.editors();
        for editor in editors {
            tracing::debug!("  📝 Registering editor: {}", editor.display_name);
            self.editor_registry.register(editor, plugin_id.clone());
        }

        // Register statusbar buttons
        let statusbar_buttons = plugin.statusbar_buttons();
        if !statusbar_buttons.is_empty() {
            tracing::debug!(
                "  🔘 Registering {} statusbar buttons",
                statusbar_buttons.len()
            );
            for button in statusbar_buttons {
                tracing::debug!("    - Button: {} at {:?}", button.id, button.position);

                // Store with plugin ID for tracking
                // SAFETY: Function pointers in buttons remain valid because
                // the plugin library is never unloaded
                self.statusbar_buttons.push((plugin_id.clone(), button));
            }

            // Sort buttons by priority within their position groups
            self.statusbar_buttons.sort_by(|(_, a), (_, b)| {
                match (&a.position, &b.position) {
                    (StatusbarPosition::Left, StatusbarPosition::Left)
                    | (StatusbarPosition::Right, StatusbarPosition::Right) => {
                        b.priority.cmp(&a.priority) // Higher priority comes first
                    }
                    (StatusbarPosition::Left, StatusbarPosition::Right) => std::cmp::Ordering::Less,
                    (StatusbarPosition::Right, StatusbarPosition::Left) => {
                        std::cmp::Ordering::Greater
                    }
                }
            });
        }

        // Collect plugin subsystems
        let subsystems = plugin.subsystems();
        if !subsystems.is_empty() {
            tracing::debug!(
                "  🧩 Registering {} subsystem(s) from plugin",
                subsystems.len()
            );
            for ss in &subsystems {
                tracing::debug!("    - Subsystem: {}", ss.id());
            }
            self.plugin_subsystems.extend(subsystems);
        }

        // Collect plugin component registrations
        let component_regs = plugin.component_factories();
        if !component_regs.is_empty() {
            tracing::debug!(
                "  🔧 Registering {} component(s) from plugin",
                component_regs.len()
            );
            for (name, _) in &component_regs {
                tracing::debug!("    - Component: {}", name);
            }
            self.plugin_component_registrations.extend(component_regs);
        }

        // Collect editor factories from the plugin
        let mut editor_registry = EditorRegistry::new();
        EditorPluginEditor::register_editors(plugin, &mut editor_registry);
        if !editor_registry.factories().is_empty() {
            tracing::debug!(
                "  📝 Registering {} editor factories",
                editor_registry.factories().len()
            );
        }

        // Store the plugin
        // SAFETY: Both plugin reference and library handle have 'static lifetime
        // because the library is never unloaded
        let loaded_plugin = LoadedPlugin {
            plugin,
            library,
            metadata: metadata.clone(),
            editor_registry,
        };

        self.plugins.insert(plugin_id.clone(), loaded_plugin);

        Ok(plugin_id)
    }

    /// Get all loaded plugins.
    pub fn get_plugins(&self) -> Vec<&PluginMetadata> {
        self.plugins.values().map(|p| &p.metadata).collect()
    }

    /// Load the plugin integrity manifest from a JSON file in the plugin directory.
    ///
    /// The manifest file must be named `plugin_integrity.json` and contains a flat
    /// JSON object mapping plugin filenames (e.g. `"my_editor.dll"`) to their
    /// expected SHA-256 hex digest strings.
    ///
    /// Special keys:
    /// - `"__allow_unlisted__": "true"` allows plugins not in the manifest to load
    ///   (use with caution).
    ///
    /// Returns `Err` if no manifest file exists (verification is required).
    fn load_plugin_manifest(
        plugin_dir: &Path,
    ) -> Result<std::collections::HashMap<String, String>, PluginManagerError> {
        let manifest_path = plugin_dir.join("plugin_integrity.json");
        if !manifest_path.exists() {
            return Err(PluginManagerError::LibraryLoadError {
                path: manifest_path,
                message: "plugin_integrity.json not found — manifest is required for security. "
                    .to_string(),
            });
        }
        let content = std::fs::read_to_string(&manifest_path).map_err(|e| {
            PluginManagerError::LibraryLoadError {
                path: manifest_path.clone(),
                message: format!("Failed to read manifest: {}", e),
            }
        })?;
        let map: std::collections::HashMap<String, String> = serde_json::from_str(&content)
            .map_err(|e| PluginManagerError::LibraryLoadError {
                path: manifest_path,
                message: format!("Invalid manifest JSON: {}", e),
            })?;
        Ok(map)
    }

    /// Parse a hex-encoded SHA-256 digest string into a 32-byte array.
    fn parse_hex_hash(hex: &str) -> Result<[u8; 32], String> {
        let hex = hex.trim();
        if hex.len() != 64 {
            return Err(format!("Expected 64 hex chars, got {}", hex.len()));
        }
        let mut out = [0u8; 32];
        for i in 0..32 {
            out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|e| format!("Invalid hex at position {}: {}", i * 2, e))?;
        }
        Ok(out)
    }

    /// Build a snapshot bridge containing AI tools exposed by all loaded plugins.
    pub fn build_tool_bridge(&self) -> PluginToolBridge {
        tracing::debug!(
            plugin_count = self.plugins.len(),
            builtin_count = self.builtin_registry.providers().len(),
            "build_tool_bridge start"
        );
        let mut bridge = PluginToolBridge::new();
        for (plugin_id, loaded_plugin) in &self.plugins {
            bridge.discover_plugin_tools(plugin_id.clone(), loaded_plugin.plugin);
        }

        for provider in self.builtin_registry.providers() {
            bridge.discover_builtin_tools(PluginId::new(provider.provider_id()), provider.clone());
        }

        // Set filesystem context from project root, if available.
        if let Some(root) = &self.project_root {
            bridge.set_fs_context(FsContext::unrestricted(root.clone()));
        }

        tracing::debug!(
            tool_count = bridge.tool_names().len(),
            "build_tool_bridge end"
        );
        bridge
    }

    /// Build a snapshot bridge containing AI tools applicable to a specific file.
    pub fn build_tool_bridge_for_file(&self, file_path: &Path) -> PluginToolBridge {
        tracing::debug!(file = %file_path.display(), plugin_count = self.plugins.len(), builtin_count = self.builtin_registry.providers().len(), "build_tool_bridge_for_file start");
        let mut bridge = PluginToolBridge::new();
        for (plugin_id, loaded_plugin) in &self.plugins {
            bridge.discover_plugin_tools_for_file(
                plugin_id.clone(),
                loaded_plugin.plugin,
                file_path,
            );
        }

        for provider in self.builtin_registry.providers() {
            bridge.discover_builtin_tools_for_file(
                PluginId::new(provider.provider_id()),
                provider.clone(),
                file_path,
            );
        }

        // Set filesystem context from project root, if available.
        if let Some(root) = &self.project_root {
            bridge.set_fs_context(FsContext::unrestricted(root.clone()));
        }

        tracing::debug!(file = %file_path.display(), tool_count = bridge.tool_names().len(), "build_tool_bridge_for_file end");
        bridge
    }

    /// Get AI tools exposed by a specific plugin or built-in provider.
    pub fn get_plugin_ai_tools(
        &self,
        plugin_id: &PluginId,
    ) -> Result<Vec<AiToolDefinition>, PluginManagerError> {
        if let Some(loaded_plugin) = self.plugins.get(plugin_id) {
            return Ok(loaded_plugin.plugin.ai_tools());
        }

        if let Some(provider) = self.builtin_registry.provider_by_id(plugin_id.as_str()) {
            return Ok(provider.ai_tools());
        }

        Err(PluginManagerError::PluginNotFound {
            plugin_id: plugin_id.clone(),
        })
    }

    /// Execute an AI tool against a specific plugin or built-in provider.
    pub fn execute_plugin_ai_tool(
        &self,
        plugin_id: &PluginId,
        file_path: &Path,
        tool_name: &str,
        tool_args: JsonValue,
    ) -> Result<JsonValue, PluginManagerError> {
        if let Some(loaded_plugin) = self.plugins.get(plugin_id) {
            return loaded_plugin
                .plugin
                .execute_ai_tool(file_path, tool_name, tool_args)
                .map_err(|error| PluginManagerError::PluginError {
                    plugin_id: plugin_id.clone(),
                    error,
                });
        }

        if let Some(provider) = self.builtin_registry.provider_by_id(plugin_id.as_str()) {
            return provider
                .execute_ai_tool(file_path, tool_name, tool_args)
                .map_err(|error| PluginManagerError::PluginError {
                    plugin_id: plugin_id.clone(),
                    error,
                });
        }

        Err(PluginManagerError::PluginNotFound {
            plugin_id: plugin_id.clone(),
        })
    }

    /// Get a reference to the file type registry.
    pub fn file_type_registry(&self) -> &FileTypeRegistry {
        &self.file_type_registry
    }

    /// Get a reference to the editor registry.
    pub fn editor_registry(&self) -> &EditorRegistry {
        &self.editor_registry
    }

    /// Get all registered statusbar buttons from all plugins.
    ///
    /// Buttons are sorted by position (left/right) and priority within each position.
    /// Higher priority buttons appear first in their respective position group.
    ///
    /// Returns only the button definitions, not the plugin IDs.
    pub fn get_statusbar_buttons(&self) -> Vec<&StatusbarButtonDefinition> {
        self.statusbar_buttons.iter().map(|(_, btn)| btn).collect()
    }

    /// Get statusbar buttons for a specific position.
    pub fn get_statusbar_buttons_for_position(
        &self,
        position: StatusbarPosition,
    ) -> Vec<&StatusbarButtonDefinition> {
        self.statusbar_buttons
            .iter()
            .filter(|(_, btn)| btn.position == position)
            .map(|(_, btn)| btn)
            .collect()
    }

    // ========================================================================
    // Component Registration (#269)
    // ========================================================================

    /// Get all component definitions registered by all plugins and built-in providers.
    pub fn get_all_component_definitions(&self) -> Vec<ComponentDefinition> {
        let mut all_defs = Vec::new();

        for loaded in self.plugins.values() {
            all_defs.extend(loaded.plugin.component_definitions());
        }

        for (_, def) in self.builtin_registry.get_all_components() {
            all_defs.push(def);
        }

        all_defs.extend(self.builtin_component_definitions.clone());

        all_defs
    }

    /// Drain all plugin-provided subsystems (consumes them for engine registration).
    ///
    /// Called once by `EngineBackend::init()` after it has built its initial
    /// registry of built-in subsystems. Plugin subsystems are merged into the
    /// engine's `SubsystemRegistry` where first-registered wins (built-in wins
    /// over plugin).
    pub fn drain_subsystems(&mut self) -> Vec<Box<dyn engine_subsystems::Subsystem>> {
        std::mem::take(&mut self.plugin_subsystems)
    }

    /// Drain all plugin-provided component registrations (consumes them for engine registration).
    ///
    /// Each entry is `(component_class_name, factory)`.
    /// Injected into `EngineBackend::plugin_components` at startup.
    pub fn drain_component_registrations(
        &mut self,
    ) -> Vec<(String, plugin_editor_api::ComponentFactory)> {
        std::mem::take(&mut self.plugin_component_registrations)
    }

    /// Create an editor instance for a file.
    ///
    /// This will:
    /// 1. Determine the file type from the path
    /// 2. Find an editor that supports that file type
    /// 3. Create an editor instance using the appropriate plugin or built-in editor
    ///
    /// # Returns
    ///
    /// Returns `Arc<dyn PanelView>` which the caller can use directly.
    ///
    /// # Safety
    ///
    /// Because plugins are never unloaded, the returned `Arc` is safe to hold
    /// indefinitely. The drop glue and vtable will remain valid for the process
    /// lifetime.
    ///
    /// # Memory Management
    ///
    /// The returned `Arc` uses normal Rust reference counting. When all references
    /// are dropped, the editor's `Drop` implementation will be called (which is safe
    /// because the plugin code is still loaded).
    ///
    /// **Avoid Arc cycles**: Use `Weak<T>` for back-references to prevent memory leaks.
    pub fn create_editor_for_file(
        &mut self,
        file_path: &Path,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginManagerError> {
        // Determine file type
        let file_type_id = self
            .file_type_registry
            .get_file_type_for_path(file_path)
            .ok_or_else(|| PluginManagerError::NoFileTypeForPath {
                path: file_path.to_path_buf(),
            })?;

        // Find an editor for this file type
        let editor_id = self
            .editor_registry
            .get_editor_for_file_type(&file_type_id)
            .ok_or_else(|| PluginManagerError::NoEditorForFileType {
                file_type_id: file_type_id.clone(),
            })?;

        // Get the plugin that owns this editor
        let plugin_id = self
            .editor_registry
            .get_plugin_for_editor(&editor_id)
            .ok_or_else(|| PluginManagerError::EditorNotFound {
                editor_id: editor_id.clone(),
            })?
            .clone(); // Clone to avoid borrow checker issues

        // Check if this is a built-in editor
        if plugin_id.as_str() == "builtin" {
            // Create editor context with project root
            let editor_context = EditorContext::new(self.project_root.clone());

            // Create the editor directly using the provider
            return self
                .builtin_registry
                .create_editor(
                    &editor_id,
                    file_path.to_path_buf(),
                    &editor_context,
                    window,
                    cx,
                )
                .map(|panel| self.decorate_editor_panel_for_path(panel, file_path))
                .map_err(|e| PluginManagerError::PluginError {
                    plugin_id,
                    error: e,
                });
        }

        // Fall back to DLL-based plugin
        self.create_editor(&plugin_id, &editor_id, file_path.to_path_buf(), window, cx)
    }

    /// Create an editor instance with a specific editor ID.
    ///
    /// # Returns
    ///
    /// Returns `Arc<dyn PanelView>` which is safe to hold indefinitely because
    /// the plugin is never unloaded.
    pub fn create_editor(
        &mut self,
        plugin_id: &PluginId,
        editor_id: &EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginManagerError> {
        let file_path_for_decoration = file_path.clone();

        let plugin =
            self.plugins
                .get_mut(plugin_id)
                .ok_or_else(|| PluginManagerError::PluginNotFound {
                    plugin_id: plugin_id.clone(),
                })?;

        // Initialize plugin globals (Theme, etc.) from main app before creating editor
        // This syncs the main app's global state into the plugin's DLL memory space
        unsafe {
            // SAFETY: We're calling an optional init function exported by the plugin.
            // This function updates the plugin's copy of global state (Theme, etc.)
            // to match the engine's current state.
            if let Ok(init_fn) = plugin
                .library
                .get::<unsafe extern "C" fn(*const std::ffi::c_void)>(b"_plugin_init_globals")
            {
                // Get Theme pointer from main app's global state
                let theme_ptr = ui::theme::Theme::global(cx) as *const _ as *const std::ffi::c_void;

                // Validate theme pointer before passing to plugin
                if !theme_ptr.is_null() {
                    init_fn(theme_ptr);
                    tracing::debug!("Initialized plugin globals for: {}", plugin_id.as_str());
                } else {
                    tracing::warn!("Theme pointer is null, plugin may not have theme access");
                }
            }
        }

        // Look up the editor factory and create the editor
        // SAFETY: The returned Arc<dyn PanelView> is safe because:
        // 1. The vtable lives in the plugin's .rodata section (never unmapped)
        // 2. The drop glue lives in the plugin's .text section (never unmapped)
        // 3. We can safely share the Arc across the boundary
        let factory = plugin
            .editor_registry
            .get(editor_id)
            .ok_or_else(|| PluginManagerError::EditorNotFound {
                editor_id: editor_id.clone(),
            })?;

        (factory.create)(file_path, window, cx)
            .map(|panel| self.decorate_editor_panel_for_path(panel, &file_path_for_decoration))
            .map_err(|e| PluginManagerError::PluginError {
                plugin_id: plugin_id.clone(),
                error: e,
            })
    }

    /// Get the default content for a file type.
    pub fn get_default_content(&self, file_type_id: &FileTypeId) -> Option<&serde_json::Value> {
        self.file_type_registry
            .get_file_type(file_type_id)
            .map(|ft| &ft.default_content)
    }

    /// Get the file type definition for a file path.
    ///
    /// This looks up the file type ID from the path and returns the full FileTypeDefinition
    /// which includes the icon and other metadata useful for UI display.
    pub fn get_file_type_for_path(
        &self,
        path: &Path,
    ) -> Option<&plugin_editor_api::FileTypeDefinition> {
        self.file_type_registry
            .get_file_type_for_path(path)
            .and_then(|id| self.file_type_registry.get_file_type(&id))
    }

    /// Create a new file of the given type.
    ///
    /// This will create the file structure on disk with default content.
    pub fn create_new_file(
        &self,
        file_type_id: &FileTypeId,
        path: &Path,
    ) -> Result<(), PluginManagerError> {
        let file_type = self
            .file_type_registry
            .get_file_type(file_type_id)
            .ok_or_else(|| PluginManagerError::FileTypeNotFound {
                file_type_id: file_type_id.clone(),
            })?;

        match &file_type.structure {
            FileStructure::Standalone => {
                // Create a simple file with default content
                let content =
                    serde_json::to_string_pretty(&file_type.default_content).map_err(|e| {
                        PluginManagerError::FileCreationError {
                            path: path.to_path_buf(),
                            message: e.to_string(),
                        }
                    })?;

                std::fs::write(path, content).map_err(|e| {
                    PluginManagerError::FileCreationError {
                        path: path.to_path_buf(),
                        message: e.to_string(),
                    }
                })?;
            }

            FileStructure::FolderBased {
                marker_file,
                template_structure,
            } => {
                // Create the folder
                std::fs::create_dir_all(path).map_err(|e| {
                    PluginManagerError::FileCreationError {
                        path: path.to_path_buf(),
                        message: e.to_string(),
                    }
                })?;

                // Create the marker file with default content
                let marker_path = path.join(marker_file);
                let content =
                    serde_json::to_string_pretty(&file_type.default_content).map_err(|e| {
                        PluginManagerError::FileCreationError {
                            path: marker_path.clone(),
                            message: e.to_string(),
                        }
                    })?;

                std::fs::write(&marker_path, content).map_err(|e| {
                    PluginManagerError::FileCreationError {
                        path: marker_path,
                        message: e.to_string(),
                    }
                })?;

                // Create template structure
                for template in template_structure {
                    match template {
                        PathTemplate::File {
                            path: rel_path,
                            content,
                        } => {
                            let file_path = path.join(rel_path);
                            if let Some(parent) = file_path.parent() {
                                std::fs::create_dir_all(parent);
                            }
                            std::fs::write(&file_path, content).map_err(|e| {
                                PluginManagerError::FileCreationError {
                                    path: file_path,
                                    message: e.to_string(),
                                }
                            })?;
                        }
                        PathTemplate::Folder { path: rel_path } => {
                            let folder_path = path.join(rel_path);
                            std::fs::create_dir_all(&folder_path).map_err(|e| {
                                PluginManagerError::FileCreationError {
                                    path: folder_path,
                                    message: e.to_string(),
                                }
                            })?;
                        }
                    }
                }
            }
        }

        Ok(())
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

// ============================================================================
// Plugin Manager Errors
// ============================================================================

/// Errors that can occur in the plugin manager.
#[derive(Debug, Clone)]
pub enum PluginManagerError {
    /// Failed to load dynamic library
    LibraryLoadError { path: PathBuf, message: String },

    /// Plugin binary integrity check failed (hash mismatch or not in manifest).
    IntegrityCheckFailed {
        path: PathBuf,
        expected: [u8; 32],
        actual: [u8; 32],
    },

    /// Required symbol not found in library
    MissingSymbol { symbol: String, message: String },

    /// Plugin version incompatible with engine
    VersionMismatch {
        expected: VersionInfo,
        actual: VersionInfo,
    },

    /// Failed to create plugin instance
    PluginCreationFailed { message: String },

    /// Plugin not found
    PluginNotFound { plugin_id: PluginId },

    /// File type not found
    FileTypeNotFound { file_type_id: FileTypeId },

    /// Editor not found
    EditorNotFound { editor_id: EditorId },

    /// No file type for path
    NoFileTypeForPath { path: PathBuf },

    /// No editor for file type
    NoEditorForFileType { file_type_id: FileTypeId },

    /// Plugin error
    PluginError {
        plugin_id: PluginId,
        error: PluginError,
    },

    /// Failed to create file
    FileCreationError { path: PathBuf, message: String },
}

impl std::fmt::Display for PluginManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LibraryLoadError { path, message } => {
                write!(f, "Failed to load library {:?}: {}", path, message)
            }
            Self::IntegrityCheckFailed {
                path,
                expected,
                actual,
            } => {
                let exp_hex = expected
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>();
                let act_hex = actual
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect::<String>();
                write!(
                    f,
                    "Integrity check failed for '{}': expected sha256={}, got sha256={}. \
                     The plugin may have been tampered with or is not in the allowlist.",
                    path.display(),
                    exp_hex,
                    act_hex,
                )
            }
            Self::MissingSymbol { symbol, message } => {
                write!(f, "Missing symbol '{}': {}", symbol, message)
            }
            Self::VersionMismatch { expected, actual } => {
                write!(
                    f,
                    "Plugin version mismatch: expected engine v{}.{}.{} with rustc hash {:#x}, got v{}.{}.{} with rustc hash {:#x}. \
                    Plugin must be recompiled with the same Rust compiler version as the engine.",
                    expected.engine_version.0,
                    expected.engine_version.1,
                    expected.engine_version.2,
                    expected.rustc_version_hash,
                    actual.engine_version.0,
                    actual.engine_version.1,
                    actual.engine_version.2,
                    actual.rustc_version_hash,
                )
            }
            Self::PluginCreationFailed { message } => {
                write!(f, "Failed to create plugin: {}", message)
            }
            Self::PluginNotFound { plugin_id } => {
                write!(f, "Plugin not found: {}", plugin_id)
            }
            Self::FileTypeNotFound { file_type_id } => {
                write!(f, "File type not found: {}", file_type_id)
            }
            Self::EditorNotFound { editor_id } => {
                write!(f, "Editor not found: {}", editor_id)
            }
            Self::NoFileTypeForPath { path } => {
                write!(f, "No file type registered for path: {:?}", path)
            }
            Self::NoEditorForFileType { file_type_id } => {
                write!(f, "No editor registered for file type: {}", file_type_id)
            }
            Self::PluginError { plugin_id, error } => {
                write!(f, "Plugin error in {}: {}", plugin_id, error)
            }
            Self::FileCreationError { path, message } => {
                write!(f, "Failed to create file {:?}: {}", path, message)
            }
        }
    }
}

impl std::error::Error for PluginManagerError {}
