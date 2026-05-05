//! # Pulsar Editor Plugin API
//!
//! This crate defines the core API for creating editor plugins that can be dynamically
//! loaded by the Pulsar engine. Plugins are compiled as dynamic libraries (.dll/.so/.dylib)
//! and loaded from the `plugins/editor/` directory at runtime.
//!
//! ## Safety Model
//!
//! This plugin system is designed to eliminate undefined behavior through **permanent library loading**:
//!
//! - **Plugins are loaded once and NEVER unloaded**
//! - Function pointers, vtables, and drop glue remain valid for process lifetime
//! - Safe to share `Arc<T>`, trait objects, and function pointers across boundary
//! - No complex weak reference workarounds needed
//!
//! ## Version Compatibility
//!
//! The plugin system uses version checking to ensure ABI compatibility:
//! - **Rustc Version**: Must match exactly (ABI is not stable across Rust versions)
//! - **Engine Major Version**: Must match (API compatibility)
//!
//! Plugins that fail version checks will be rejected at load time.
//!
//! ## Creating a Plugin
//!
//! 1. Create a new crate with `crate-type = ["cdylib"]`
//! 2. Add dependency on `plugin_editor_api` (same version as engine!)
//! 3. Implement the `EditorPlugin` trait
//! 4. Use the `export_plugin!` macro to export your plugin
//! 5. Build as a dynamic library
//! 6. Place the .dll/.so/.dylib in `plugins/editor/`
//!
//! ## Example
//!
//! ```rust,ignore
//! use plugin_editor_api::*;
//!
//! #[derive(Default)]
//! struct MyEditorPlugin;
//!
//! impl EditorPlugin for MyEditorPlugin {
//!     fn metadata(&self) -> PluginMetadata {
//!         PluginMetadata {
//!             id: PluginId::new("com.example.my-editor"),
//!             name: "My Editor".into(),
//!             version: "1.0.0".into(),
//!             author: "Example Corp".into(),
//!             description: "An example editor plugin".into(),
//!         }
//!     }
//!
//!     fn file_types(&self) -> Vec<FileTypeDefinition> {
//!         vec![FileTypeDefinition {
//!             id: FileTypeId::new("my-file"),
//!             extension: "myfile".into(),
//!             display_name: "My File".into(),
//!             icon: ui::IconName::Code,
//!             color: gpui::rgb(0x2196F3),
//!             structure: FileStructure::Standalone,
//!             default_content: serde_json::json!({"version": 1}),
//!             categories: vec![],
//!         }]
//!     }
//!
//!     fn editors(&self) -> Vec<EditorMetadata> {
//!         vec![EditorMetadata {
//!             id: EditorId::new("my-editor"),
//!             display_name: "My Editor".into(),
//!             supported_file_types: vec![FileTypeId::new("my-file")],
//!         }]
//!     }
//!
//!     fn create_editor(
//!         &self,
//!         editor_id: EditorId,
//!         file_path: PathBuf,
//!         window: &mut Window,
//!         cx: &mut App,
//!     ) -> Result<Arc<dyn ui::dock::PanelView>, PluginError> {
//!         // Create and return your editor instance
//!         // Safe to return Arc directly because plugin is never unloaded!
//!         Ok(Arc::new(MyEditor::new(file_path, window, cx)?))
//!     }
//! }
//!
//! export_plugin!(MyEditorPlugin);
//! ```
//!
//! ## Memory Management
//!
//! ### Arc Sharing (Safe!)
//!
//! Unlike typical DLL boundaries, you can safely share `Arc<T>` because plugins are never unloaded:
//!
//! ```rust,ignore
//! // Plugin creates Arc with drop glue in plugin DLL:
//! let panel = Arc::new(MyPanel::new());
//!
//! // Engine receives Arc and holds it:
//! return Ok(panel);  // ✅ Safe! Plugin DLL never unloads
//!
//! // Later, when engine drops the Arc:
//! drop(panel);  // ✅ Safe! Drop glue still valid in plugin DLL
//! ```
//!
//! ### Preventing Arc Cycles
//!
//! While plugins are never unloaded (preventing UB), you can still create memory leaks with `Arc` cycles.
//! Use `Weak<T>` to break cycles:
//!
//! ```rust,ignore
//! struct Workspace {
//!     tabs: Vec<Arc<Tab>>,  // Parent owns children
//! }
//!
//! struct Tab {
//!     workspace: Weak<Workspace>,  // Child weakly references parent (breaks cycle)
//! }
//! ```
//!
//! See `PLUGIN_ARCHITECTURE.md` for detailed guidance on Arc cycle prevention.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

pub mod asset_payload;
pub use asset_payload::{AssetDropArea, AssetDropAreaExt, AssetPayload};
// Re-export AssetKind at crate root for convenience.
pub use ui_types_common::AssetKind;

pub use gpui::{App, Window};
pub use ui::dock::{Panel, PanelView};

// Re-export for plugins to use
pub use serde_json::Value as JsonValue;

// ============================================================================
// Statusbar Button System
// ============================================================================

/// Represents the position where a statusbar button should be placed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusbarPosition {
    /// Left side of the statusbar (with drawer buttons)
    Left,
    /// Right side of the statusbar (with analyzer status)
    Right,
}

/// Action to perform when a statusbar button is clicked
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StatusbarAction {
    /// Open an editor by its EditorId in the tab system
    OpenEditor {
        editor_id: EditorId,
        /// Optional file path to open. If None, creates a new empty editor.
        file_path: Option<PathBuf>,
    },

    /// Toggle visibility of a drawer/panel
    ToggleDrawer {
        /// Unique identifier for the drawer
        drawer_id: String,
    },

    /// Execute a custom callback (function pointer provided by plugin)
    /// The callback receives (Window, App) and can perform any action
    ///
    /// # Safety
    ///
    /// Because plugins are never unloaded, function pointers remain valid
    /// for the process lifetime. This is safe!
    Custom,
}

/// Unique identifier for a statusbar button
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StatusbarButtonId(String);

impl StatusbarButtonId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StatusbarButtonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Definition of a statusbar button that a plugin can register
#[derive(Clone)]
pub struct StatusbarButtonDefinition {
    /// Unique identifier for this button
    pub id: StatusbarButtonId,

    /// Icon to display
    pub icon: ui::IconName,

    /// Tooltip text shown on hover
    pub tooltip: String,

    /// Position in the statusbar
    pub position: StatusbarPosition,

    /// Optional badge count to display (e.g., error count)
    pub badge_count: Option<u32>,

    /// Optional badge color (if None, uses default theme color)
    pub badge_color: Option<gpui::Hsla>,

    /// Action to perform when clicked
    pub action: StatusbarAction,

    /// Optional custom callback for Custom action type
    ///
    /// # Safety
    ///
    /// This function pointer remains valid because plugins are never unloaded.
    /// The plugin code stays loaded for the process lifetime, so this pointer
    /// will always point to valid code.
    pub custom_callback: Option<fn(&mut Window, &mut App)>,

    /// Priority for ordering (higher = further right/left, depending on position)
    pub priority: i32,

    /// Whether the button is currently active/selected
    pub active: bool,

    /// Optional custom color for the icon
    pub icon_color: Option<gpui::Hsla>,
}

impl StatusbarButtonDefinition {
    /// Create a new statusbar button definition
    pub fn new(
        id: impl Into<String>,
        icon: ui::IconName,
        tooltip: impl Into<String>,
        position: StatusbarPosition,
        action: StatusbarAction,
    ) -> Self {
        Self {
            id: StatusbarButtonId::new(id),
            icon,
            tooltip: tooltip.into(),
            position,
            badge_count: None,
            badge_color: None,
            action,
            custom_callback: None,
            priority: 0,
            active: false,
            icon_color: None,
        }
    }

    /// Set the badge count
    pub fn with_badge(mut self, count: u32) -> Self {
        self.badge_count = Some(count);
        self
    }

    /// Set the badge color
    pub fn with_badge_color(mut self, color: gpui::Hsla) -> Self {
        self.badge_color = Some(color);
        self
    }

    /// Set the custom callback (for Custom action type)
    pub fn with_callback(mut self, callback: fn(&mut Window, &mut App)) -> Self {
        self.custom_callback = Some(callback);
        self
    }

    /// Set the priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set whether the button is active
    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Set a custom icon color
    pub fn with_icon_color(mut self, color: gpui::Hsla) -> Self {
        self.icon_color = Some(color);
        self
    }
}

// ============================================================================
// Version Information
// ============================================================================

/// Version information for compatibility checking across the DLL boundary.
///
/// This struct ensures that plugins are loaded only if they were compiled with
/// compatible versions of the engine and Rust compiler.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Engine version (major, minor, patch)
    pub engine_version: (u32, u32, u32),
    /// Rustc version hash (hash of semver part only)
    pub rustc_version_hash: u64,
}

impl VersionInfo {
    /// Get the current version info for this build
    pub const fn current() -> Self {
        Self {
            engine_version: parse_engine_version(),
            rustc_version_hash: rustc_version_hash(),
        }
    }

    /// Check if two versions are compatible
    pub fn is_compatible(&self, other: &Self) -> bool {
        // Engine major version must match
        if self.engine_version.0 != other.engine_version.0 {
            return false;
        }

        // Rustc version must match exactly (ABI not stable across versions)
        if self.rustc_version_hash != other.rustc_version_hash {
            return false;
        }

        true
    }
}

/// Compile-time hash of the rustc version
/// This is set at compile time to ensure ABI compatibility
const fn rustc_version_hash() -> u64 {
    // Extract and hash only the semver part (e.g., "1.83.0" from "rustc 1.83.0 (90b35a623 2024-11-26)")
    // This ensures compatibility is based on the actual compiler version, not build metadata
    const RUSTC_VERSION: &str = env!("RUSTC_VERSION");
    hash_semver_only(RUSTC_VERSION)
}

/// Hash only the semver portion of rustc version string
/// e.g., "1.83.0" from "rustc 1.83.0 (90b35a623 2024-11-26)"
const fn hash_semver_only(version: &str) -> u64 {
    let bytes = version.as_bytes();
    let mut start = 0;
    let mut end = 0;
    let mut found_start = false;
    let mut i = 0;

    // Find the first digit (start of version)
    while i < bytes.len() {
        if bytes[i] >= b'0' && bytes[i] <= b'9' && !found_start {
            start = i;
            found_start = true;
        }
        // Find the first space or '(' after the version (end of semver)
        if found_start && (bytes[i] == b' ' || bytes[i] == b'(') {
            end = i;
            break;
        }
        i += 1;
    }

    if end == 0 {
        end = bytes.len();
    }

    // Hash only the bytes in the semver range [start..end)
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    let mut i = start;
    while i < end {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
        i += 1;
    }
    hash
}

/// Parse engine version from CARGO_PKG_VERSION at compile time
/// Expects format "major.minor.patch" e.g. "0.1.47"
const fn parse_engine_version() -> (u32, u32, u32) {
    const VERSION_STR: &str = env!("CARGO_PKG_VERSION");
    let bytes = VERSION_STR.as_bytes();

    let mut major: u32 = 0;
    let mut minor: u32 = 0;
    let mut patch: u32 = 0;
    let mut component = 0; // 0 = major, 1 = minor, 2 = patch
    let mut i = 0;

    while i < bytes.len() {
        let byte = bytes[i];
        if byte == b'.' {
            component += 1;
        } else if byte >= b'0' && byte <= b'9' {
            let digit = (byte - b'0') as u32;
            match component {
                0 => major = major * 10 + digit,
                1 => minor = minor * 10 + digit,
                2 => patch = patch * 10 + digit,
                _ => {}
            }
        }
        i += 1;
    }

    (major, minor, patch)
}

// ============================================================================
// Plugin Identification
// ============================================================================

/// Unique identifier for a plugin.
///
/// Uses reverse domain notation (e.g., "com.pulsar.blueprint-editor")
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(String);

impl PluginId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a file type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileTypeId(String);

impl FileTypeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FileTypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for an editor type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EditorId(String);

impl EditorId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EditorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ============================================================================
// Plugin Metadata
// ============================================================================

/// Metadata describing a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMetadata {
    /// Unique plugin identifier (reverse domain notation)
    pub id: PluginId,
    /// Human-readable plugin name
    pub name: String,
    /// Plugin version (semantic versioning recommended)
    pub version: String,
    /// Plugin author/organization
    pub author: String,
    /// Brief description of the plugin
    pub description: String,
}

// ============================================================================
// File Type Definitions
// ============================================================================

/// Defines the structure of a file type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileStructure {
    /// A single standalone file (e.g., script.rs)
    Standalone,

    /// A folder that appears as a file in the drawer (e.g., MyClass.class/)
    /// Contains the marker file name that identifies this folder as this type
    /// For example, "graph_save.json" for blueprint classes
    FolderBased {
        /// The marker file that must exist in the folder
        marker_file: String,
        /// Additional files/folders that should be created in a new instance
        template_structure: Vec<PathTemplate>,
    },
}

/// Template for creating files/folders in a folder-based file type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PathTemplate {
    /// Create a file with default content
    File { path: String, content: String },
    /// Create a folder
    Folder { path: String },
}

/// Complete definition of a file type that a plugin supports.
#[derive(Debug, Clone)]
pub struct FileTypeDefinition {
    /// Unique identifier for this file type
    pub id: FileTypeId,

    /// File extension (without the dot, e.g., "rs" not ".rs")
    /// For folder-based files, this is the folder extension
    pub extension: String,

    /// Human-readable name for this file type
    pub display_name: String,

    /// Icon to show in the file drawer
    pub icon: ui::IconName,

    /// Color for the icon
    pub color: gpui::Hsla,

    /// Whether this is a standalone file or folder-based
    pub structure: FileStructure,

    /// Default content for new files (as JSON)
    /// For folder-based files, this is the content of the marker file
    pub default_content: serde_json::Value,

    /// Optional category path for organizing in the create menu
    /// Examples: vec!["Data"], vec!["Data", "SQLite"], vec!["Scripts", "Web"]
    /// Leave empty for top-level menu items
    pub categories: Vec<String>,
}

// ============================================================================
// Editor Metadata
// ============================================================================

/// Metadata describing an editor that a plugin provides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorMetadata {
    /// Unique identifier for this editor
    pub id: EditorId,

    /// Human-readable name for this editor
    pub display_name: String,

    /// List of file type IDs that this editor can open
    pub supported_file_types: Vec<FileTypeId>,
}

// ============================================================================
// Plugin Error
// ============================================================================

/// Errors that can occur in plugin operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginError {
    /// Failed to load file
    FileLoadError { path: PathBuf, message: String },

    /// Failed to save file
    FileSaveError { path: PathBuf, message: String },

    /// Invalid file format
    InvalidFormat { expected: String, message: String },

    /// Editor not found
    EditorNotFound { editor_id: EditorId },

    /// File type not supported
    UnsupportedFileType { file_type_id: FileTypeId },

    /// Version mismatch
    VersionMismatch {
        expected: VersionInfo,
        actual: VersionInfo,
    },

    /// Generic error
    Other { message: String },
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileLoadError { path, message } => {
                write!(f, "Failed to load file {:?}: {}", path, message)
            }
            Self::FileSaveError { path, message } => {
                write!(f, "Failed to save file {:?}: {}", path, message)
            }
            Self::InvalidFormat { expected, message } => {
                write!(f, "Invalid format (expected {}): {}", expected, message)
            }
            Self::EditorNotFound { editor_id } => {
                write!(f, "Editor not found: {}", editor_id)
            }
            Self::UnsupportedFileType { file_type_id } => {
                write!(f, "Unsupported file type: {}", file_type_id)
            }
            Self::VersionMismatch { expected, actual } => {
                write!(
                    f,
                    "Version mismatch: expected {:?}, got {:?}",
                    expected, actual
                )
            }
            Self::Other { message } => write!(f, "{}", message),
        }
    }
}

impl std::error::Error for PluginError {}

// ============================================================================
// Core Plugin Trait
// ============================================================================

/// The main trait that all editor plugins must implement.
///
/// This trait is loaded dynamically from compiled plugin libraries and provides
/// all the information and functionality needed to integrate the plugin with
/// the Pulsar engine.
///
/// # Safety Contract
///
/// Because plugins are **never unloaded**, implementors can safely:
/// - Return `Arc<T>` where `T`'s drop glue is in the plugin
/// - Return trait objects whose vtables are in the plugin
/// - Store function pointers in returned data structures
///
/// All of these remain valid for the process lifetime.
///
/// # Requirements
///
/// Implementors must ensure:
/// - The plugin is compiled with the same Rust version as the engine
/// - The plugin uses the same version of shared dependencies (plugin_editor_api, gpui, ui)
/// - All returned data is valid and properly initialized
pub trait EditorPlugin: Send + Sync {
    /// Get the version information for this plugin.
    ///
    /// This is checked against the engine's version before loading.
    fn version_info(&self) -> VersionInfo {
        VersionInfo::current()
    }

    /// Get metadata about this plugin.
    fn metadata(&self) -> PluginMetadata;

    /// Get all file types this plugin supports.
    fn file_types(&self) -> Vec<FileTypeDefinition>;

    /// Get all editor types this plugin provides.
    fn editors(&self) -> Vec<EditorMetadata>;

    /// Create a new editor instance for the given file.
    ///
    /// # Arguments
    ///
    /// * `editor_id` - The ID of the editor to create
    /// * `file_path` - The path to the file to open
    /// * `window` - The window to create the editor in
    /// * `cx` - The application context
    ///
    /// # Returns
    ///
    /// Returns an `Arc<dyn PanelView>` that the engine will manage.
    ///
    /// # Safety
    ///
    /// It is safe to return `Arc<dyn PanelView>` because the plugin is never unloaded.
    /// The drop glue and vtable will remain valid for the process lifetime.
    ///
    /// # Memory Management
    ///
    /// The Arc refcount will keep the panel alive. When all references are dropped,
    /// the panel's Drop implementation will be called (which is safe because the
    /// plugin code is still loaded).
    ///
    /// **Avoid Arc cycles**: Use `Weak<T>` for back-references to prevent memory leaks.
    fn create_editor(
        &self,
        editor_id: EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError>;

    /// Called when the plugin is loaded.
    ///
    /// Use this for any initialization that needs to happen once.
    fn on_load(&mut self) {}

    /// Get statusbar buttons this plugin wants to register.
    ///
    /// This is optional - plugins that don't need statusbar buttons can use the default implementation.
    ///
    /// # Returns
    ///
    /// A vector of statusbar button definitions. Return an empty vector if no buttons are needed.
    ///
    /// # Safety
    ///
    /// It is safe to return function pointers in `StatusbarButtonDefinition` because
    /// plugins are never unloaded. The function pointers will remain valid for the
    /// process lifetime.
    fn statusbar_buttons(&self) -> Vec<StatusbarButtonDefinition> {
        Vec::new()
    }

    /// Declare which [`AssetKind`]s this plugin's editors are willing to accept
    /// when an asset is dropped onto one of their panels.
    ///
    /// The engine uses this at runtime to show/hide drop-accept indicators before
    /// the user releases the drag. Returning an empty vec means the plugin does
    /// not participate in asset drops.
    fn accepted_drop_kinds(&self) -> Vec<AssetKind> {
        Vec::new()
    }
}

// ============================================================================
// Plugin Declaration and Export
// ============================================================================

/// Type alias for the plugin constructor function.
///
/// Plugins must export a function with this signature named `_plugin_create`.
///
/// # Safety
///
/// The returned reference has `'static` lifetime because the plugin is never unloaded.
/// This is safe because the plugin code remains valid for the process lifetime.
pub type PluginCreate =
    unsafe extern "C" fn(theme_ptr: *const std::ffi::c_void) -> &'static mut dyn EditorPlugin;

/// Macro to export a plugin from a dynamic library.
///
/// This generates the necessary FFI functions for the plugin to be loaded
/// by the engine.
///
/// # Safety
///
/// This macro generates `unsafe extern "C"` functions. The safety contract is:
///
/// 1. **Plugin is never unloaded**: The returned `&'static mut dyn EditorPlugin` reference
///    is valid forever because we never call dlclose/FreeLibrary.
///
/// 2. **Theme pointer validity**: The engine must ensure the Theme pointer remains valid.
///    This is guaranteed by the engine keeping Theme in stable storage.
///
/// 3. **Version checking**: Plugins must be built with compatible versions (checked at runtime).
///
/// # Example
///
/// ```rust,ignore
/// struct MyPlugin;
/// impl EditorPlugin for MyPlugin { /* ... */ }
///
/// export_plugin!(MyPlugin);
/// ```
#[macro_export]
macro_rules! export_plugin {
    ($plugin_type:ty) => {
        // Static storage for synced Theme data from main app
        //
        // SAFETY CONTRACT: The main app MUST ensure the Theme pointer remains valid for the
        // entire lifetime of the process. This is guaranteed by storing Theme in static
        // storage in the engine.
        static SYNCED_THEME: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

        /// Create the plugin instance.
        ///
        /// # Safety
        ///
        /// This function returns a `&'static mut dyn EditorPlugin` which is safe because:
        /// 1. The plugin is never unloaded (PermanentLibrary prevents dlclose/FreeLibrary)
        /// 2. The returned reference is leaked intentionally
        /// 3. All plugin code remains valid for process lifetime
        ///
        /// The theme_ptr must point to valid Theme data that remains stable for the process
        /// lifetime. This is the caller's (engine's) responsibility.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_create(
            theme_ptr: *const std::ffi::c_void,
        ) -> &'static mut dyn $crate::EditorPlugin {
            // Validate theme pointer
            if theme_ptr.is_null() {
                panic!("[Plugin] ERROR: Received null theme pointer from host!");
            }

            // Store theme pointer (fail if already set)
            if SYNCED_THEME.set(theme_ptr as usize).is_err() {
                panic!("[Plugin] ERROR: Theme pointer already initialized!");
            }

            // Register our theme accessor with the ui crate
            ui::theme::Theme::register_plugin_accessor(plugin_theme_unsafe);

            // Create the plugin instance
            let plugin = <$plugin_type>::default();
            let boxed: Box<dyn $crate::EditorPlugin> = Box::new(plugin);

            // Leak the box to get a 'static reference
            //
            // SAFETY: This is intentional. Because the plugin is never unloaded:
            // 1. This memory will be freed when the process exits (OS cleanup)
            // 2. The reference remains valid for process lifetime
            // 3. No use-after-free is possible
            Box::leak(boxed)
        }

        /// Internal accessor for plugin theme (called by ui crate).
        ///
        /// # Safety
        ///
        /// SAFETY: Returns None if theme pointer is null or not initialized.
        /// The caller (ui crate) must handle None gracefully.
        ///
        /// The returned reference is valid because:
        /// 1. The engine guarantees Theme stays in stable storage
        /// 2. Plugins are never unloaded
        /// 3. We validate the pointer is not null before dereferencing
        unsafe fn plugin_theme_unsafe() -> Option<&'static ui::theme::Theme> {
            let ptr = SYNCED_THEME.get().copied()? as *const std::ffi::c_void;

            // Validate pointer is not null before dereferencing
            if ptr.is_null() {
                return None;
            }

            // SAFETY: The engine contract guarantees this pointer remains valid.
            // We never unload the plugin, so this reference is 'static.
            Some(&*(ptr as *const ui::theme::Theme))
        }

        /// Get the plugin's version information.
        ///
        /// This is called before plugin creation to verify compatibility.
        #[no_mangle]
        pub extern "C" fn _plugin_version() -> $crate::VersionInfo {
            $crate::VersionInfo::current()
        }

        /// Initialize the plugin's globals from the main app.
        ///
        /// This is called before each editor instance creation to ensure fresh state.
        ///
        /// # Safety
        ///
        /// The theme_ptr must point to valid Theme data that remains stable for the
        /// process lifetime.
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_init_globals(theme_ptr: *const std::ffi::c_void) {
            // Validate theme pointer
            if theme_ptr.is_null() {
                tracing::error!("[Plugin] ERROR: Received null theme pointer in init_globals!");
                return;
            }

            // OnceLock.set() will fail if already set, which is fine.
            // The theme pointer should remain stable across the plugin lifetime.
            if SYNCED_THEME.get().is_none() {
                SYNCED_THEME.set(theme_ptr as usize).ok();
            }
        }
    };
}

// ============================================================================
// Helper Utilities
// ============================================================================

/// Helper to create a simple standalone file type definition.
pub fn standalone_file_type(
    id: impl Into<String>,
    extension: impl Into<String>,
    display_name: impl Into<String>,
    icon: ui::IconName,
    color: gpui::Hsla,
    default_content: serde_json::Value,
) -> FileTypeDefinition {
    FileTypeDefinition {
        id: FileTypeId::new(id),
        extension: extension.into(),
        display_name: display_name.into(),
        icon,
        color,
        structure: FileStructure::Standalone,
        default_content,
        categories: vec![],
    }
}

/// Helper to create a folder-based file type definition.
pub fn folder_file_type(
    id: impl Into<String>,
    extension: impl Into<String>,
    display_name: impl Into<String>,
    icon: ui::IconName,
    color: gpui::Hsla,
    marker_file: impl Into<String>,
    template_structure: Vec<PathTemplate>,
    default_content: serde_json::Value,
) -> FileTypeDefinition {
    FileTypeDefinition {
        id: FileTypeId::new(id),
        extension: extension.into(),
        display_name: display_name.into(),
        icon,
        color,
        structure: FileStructure::FolderBased {
            marker_file: marker_file.into(),
            template_structure,
        },
        default_content,
        categories: vec![],
    }
}
