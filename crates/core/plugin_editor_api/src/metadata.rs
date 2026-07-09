use serde::{Deserialize, Serialize};

use crate::identifiers::{EditorId, FileTypeId, PluginId};

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
