//! [`PluginManagerError`], the error type of every fallible manager call.

use plugin_editor_api::*;
use std::path::PathBuf;

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

    /// The plugin's world crates refused the editor's world runtimes: the
    /// library was built from other world-crate sources or features.
    WorldRuntimeRefused { path: PathBuf },

    /// Failed to create plugin instance
    PluginCreationFailed { message: String },

    /// The plugin's dependencies are not met: missing, out of range, cyclic,
    /// or a dependency did not load.
    DependencyUnmet { plugin: PluginId, reason: String },

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
            Self::WorldRuntimeRefused { path } => write!(
                f,
                "Plugin '{}' refused the editor's world runtimes: it was built from other \
                 SceneDB or world-registry sources or features. Rebuild it against this engine.",
                path.display()
            ),
            Self::PluginCreationFailed { message } => {
                write!(f, "Failed to create plugin: {}", message)
            }
            Self::DependencyUnmet { plugin, reason } => {
                write!(f, "Plugin {plugin} not loaded: it {reason}")
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
