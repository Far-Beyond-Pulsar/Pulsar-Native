use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

use crate::identifiers::{EditorId, FileTypeId};
use crate::version::VersionInfo;

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

    /// Filesystem access denied by FsContext sandbox.
    AccessDenied(String),
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
            Self::AccessDenied(reason) => write!(f, "Access denied: {}", reason),
        }
    }
}

impl std::error::Error for PluginError {}
