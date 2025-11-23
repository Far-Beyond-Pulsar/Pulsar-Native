//! # Pulsar Editor Plugin API
//!
//! This crate defines the core API for creating editor plugins that can be dynamically
//! loaded by the Pulsar engine. Plugins are compiled as dynamic libraries (.dll/.so/.dylib)
//! and loaded from the `plugins/editor/` directory at runtime.
//!
//! ## Safety and Versioning
//!
//! The plugin system uses version checking to ensure ABI compatibility:
//! - **Engine Version**: Ensures the plugin was built for the correct engine version
//! - **Rustc Version**: Ensures the plugin was compiled with the same Rust compiler
//!
//! Plugins that fail version checks will be rejected at load time.
//!
//! ## Creating a Plugin
//!
//! 1. Create a new crate with `crate-type = ["cdylib"]`
//! 2. Implement the `EditorPlugin` trait
//! 3. Use the `export_plugin!` macro to export your plugin
//! 4. Build as a dynamic library
//! 5. Place the .dll/.so/.dylib in `plugins/editor/`
//!
//! ## Example
//!
//! ```rust,ignore
//! use plugin_editor_api::*;
//!
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
//!             icon: FileIcon::Code,
//!             structure: FileStructure::Standalone,
//!             default_content: serde_json::json!({"version": 1}),
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
//!     ) -> Result<Box<dyn EditorInstance>, PluginError> {
//!         // Create and return your editor instance
//!         Ok(Box::new(MyEditor::new(file_path, window, cx)?))
//!     }
//! }
//!
//! export_plugin!(MyEditorPlugin);
//! ```

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

pub use gpui::{App, Window};
pub use ui::dock::Panel;

// Re-export for plugins to use
pub use serde_json::Value as JsonValue;

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
    /// Rustc version hash (first 8 bytes of version string hash)
    pub rustc_version_hash: u64,
}

impl VersionInfo {
    /// Get the current version info for this build
    pub const fn current() -> Self {
        Self {
            engine_version: (0, 1, 0), // TODO: Read from engine version
            rustc_version_hash: rustc_version_hash(),
        }
    }

    /// Check if two versions are compatible
    pub fn is_compatible(&self, other: &Self) -> bool {
        // Engine major version must match
        if self.engine_version.0 != other.engine_version.0 {
            return false;
        }

        // Rustc version must match exactly
        if self.rustc_version_hash != other.rustc_version_hash {
            return false;
        }

        true
    }
}

/// Compile-time hash of the rustc version
const fn rustc_version_hash() -> u64 {
    // Using rustc version from env - fallback if not set
    // In build.rs, we set this. For now, use a const value
    // TODO: Set via build.rs
    const RUSTC_VERSION: &str = env!("CARGO_PKG_RUST_VERSION", "unknown");
    const_hash_str(RUSTC_VERSION)
}

/// Simple const hash function for version string
const fn const_hash_str(s: &str) -> u64 {
    let bytes = s.as_bytes();
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV-1a prime
        i += 1;
    }
    hash
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
    File {
        path: String,
        content: String,
    },
    /// Create a folder
    Folder {
        path: String,
    },
}

/// Icon to display for a file type in the file drawer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileIcon {
    // Common icons
    File,
    Code,
    Component,
    Database,
    Music,
    Image,
    Video,
    Audio,
    Archive,
    Document,

    // Programming language icons
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Cpp,
    CSharp,
    Go,

    // Asset types
    Model3D,
    Texture,
    Material,
    Animation,
    Particle,
    Level,
    Prefab,

    // Type system
    Struct,
    Enum,
    Trait,
    Interface,
    Class,

    // Custom icon (base64 encoded PNG/SVG)
    Custom(String),
}

/// Complete definition of a file type that a plugin supports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTypeDefinition {
    /// Unique identifier for this file type
    pub id: FileTypeId,

    /// File extension (without the dot, e.g., "rs" not ".rs")
    /// For folder-based files, this is the folder extension
    pub extension: String,

    /// Human-readable name for this file type
    pub display_name: String,

    /// Icon to show in the file drawer
    pub icon: FileIcon,

    /// Whether this is a standalone file or folder-based
    pub structure: FileStructure,

    /// Default content for new files (as JSON)
    /// For folder-based files, this is the content of the marker file
    pub default_content: serde_json::Value,
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
// Editor Instance
// ============================================================================

/// Trait for editor instances created by plugins.
///
/// **Important**: Editor instances must ALSO implement the following traits
/// for tab system integration:
/// - `Panel` - Core panel interface
/// - `Render` - UI rendering
/// - `FocusableView` - Keyboard focus handling
/// - `EventEmitter<PanelEvent>` - Tab lifecycle events
///
/// These cannot be enforced by the trait system due to Rust's trait object
/// limitations, but the engine expects them to be implemented.
pub trait EditorInstance: Send + Sync {
    /// Get the file path this editor is editing
    fn file_path(&self) -> &PathBuf;

    /// Save the current state to disk
    fn save(&mut self, window: &mut Window, cx: &mut App) -> Result<(), PluginError>;

    /// Reload from disk
    fn reload(&mut self, window: &mut Window, cx: &mut App) -> Result<(), PluginError>;

    /// Check if the editor has unsaved changes
    fn is_dirty(&self) -> bool;

    /// Get the underlying wrapper as Any for downcasting
    /// This allows the application to access plugin-specific functionality
    fn as_any(&self) -> &dyn std::any::Any;
}

// ============================================================================
// Plugin Error
// ============================================================================

/// Errors that can occur in plugin operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginError {
    /// Failed to load file
    FileLoadError {
        path: PathBuf,
        message: String,
    },

    /// Failed to save file
    FileSaveError {
        path: PathBuf,
        message: String,
    },

    /// Invalid file format
    InvalidFormat {
        expected: String,
        message: String,
    },

    /// Editor not found
    EditorNotFound {
        editor_id: EditorId,
    },

    /// File type not supported
    UnsupportedFileType {
        file_type_id: FileTypeId,
    },

    /// Version mismatch
    VersionMismatch {
        expected: VersionInfo,
        actual: VersionInfo,
    },

    /// Generic error
    Other {
        message: String,
    },
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
/// # Safety
///
/// Implementors must ensure:
/// - The plugin is compiled with the same Rust version as the engine
/// - All returned data is valid and properly initialized
/// - Editor instances are properly constructed and cleaned up
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
    /// Returns the editor wrapped for tab system integration, plus the EditorInstance for file operations.
    /// The panel can be added directly to the tab system.
    fn create_editor(
        &self,
        editor_id: EditorId,
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(std::sync::Arc<dyn ui::dock::PanelView>, Box<dyn EditorInstance>), PluginError>;

    /// Called when the plugin is loaded.
    ///
    /// Use this for any initialization that needs to happen once.
    fn on_load(&mut self) {}

    /// Called when the plugin is unloaded.
    ///
    /// Use this for cleanup.
    fn on_unload(&mut self) {}
}

// ============================================================================
// Plugin Declaration and Export
// ============================================================================

/// Type alias for the plugin constructor function.
///
/// Plugins must export a function with this signature named `_plugin_create`.
pub type PluginCreate = unsafe extern "C" fn() -> *mut dyn EditorPlugin;

/// Type alias for the plugin destructor function.
///
/// Plugins must export a function with this signature named `_plugin_destroy`.
pub type PluginDestroy = unsafe extern "C" fn(*mut dyn EditorPlugin);

/// Macro to export a plugin from a dynamic library.
///
/// This generates the necessary FFI functions for the plugin to be loaded
/// by the engine.
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
        #[no_mangle]
        pub unsafe extern "C" fn _plugin_create() -> *mut dyn $crate::EditorPlugin {
            let plugin = <$plugin_type>::default();
            let boxed: Box<dyn $crate::EditorPlugin> = Box::new(plugin);
            Box::into_raw(boxed)
        }

        #[no_mangle]
        pub unsafe extern "C" fn _plugin_destroy(ptr: *mut dyn $crate::EditorPlugin) {
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }

        #[no_mangle]
        pub extern "C" fn _plugin_version() -> $crate::VersionInfo {
            $crate::VersionInfo::current()
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
    icon: FileIcon,
    default_content: serde_json::Value,
) -> FileTypeDefinition {
    FileTypeDefinition {
        id: FileTypeId::new(id),
        extension: extension.into(),
        display_name: display_name.into(),
        icon,
        structure: FileStructure::Standalone,
        default_content,
    }
}

/// Helper to create a folder-based file type definition.
pub fn folder_file_type(
    id: impl Into<String>,
    extension: impl Into<String>,
    display_name: impl Into<String>,
    icon: FileIcon,
    marker_file: impl Into<String>,
    template_structure: Vec<PathTemplate>,
    default_content: serde_json::Value,
) -> FileTypeDefinition {
    FileTypeDefinition {
        id: FileTypeId::new(id),
        extension: extension.into(),
        display_name: display_name.into(),
        icon,
        structure: FileStructure::FolderBased {
            marker_file: marker_file.into(),
            template_structure,
        },
        default_content,
    }
}
